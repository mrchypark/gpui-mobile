//! Async BLE central/GATT client. See `docs/ble-integration.md` for host setup.
//! All methods must be polled on a Tokio runtime, never blocked on the UI thread.

use btleplug::api::{Central, Manager as _, Peripheral as _};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::stream::BoxStream;
use std::{collections::BTreeSet, future::Future, time::Duration};

pub use btleplug::api::{
    CentralEvent, CentralState, CharPropFlags, Characteristic, PeripheralProperties, ScanFilter,
    Service, ValueNotification, WriteType,
};
pub use btleplug::platform::PeripheralId;
pub use btleplug::{Error, Result};

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::init_android;
mod state;
mod convenience;
mod background;
pub use convenience::{ScanOptions, ScanResult};
pub mod demo;
pub mod qcy;
#[cfg(all(target_os = "android", debug_assertions, feature = "location", feature = "permission_handler"))]
mod device_probe;
pub use state::Permission;
use state::State;

/// One central session and one active peripheral. Create separate sessions only
/// if the application deliberately coordinates adapter-wide scan ownership.
/// Await `pause`/`close` before dropping. Drop only schedules best-effort cleanup
/// on the creating runtime, which must remain running until cleanup completes.
pub struct Ble {
    adapter: Adapter,
    peripheral: Option<Peripheral>,
    state: State,
    scanning: bool,
    discovered: BTreeSet<Characteristic>,
    timeout: Duration,
    runtime: tokio::runtime::Handle,
    background: Option<background::Lease>,
    in_background: bool,
}

impl Ble {
    /// Permissions are supplied by the application's native permission callback.
    /// `Granted` is not a substitute for OS permission; native errors propagate.
    pub async fn new(permission: Permission, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::NotSupported("BLE timeout must be nonzero".into()));
        }
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| Error::RuntimeError(format!("BLE needs a Tokio runtime: {e}")))?;
        #[cfg(target_os = "android")]
        android::ensure_initialized()?;
        let adapter = deadline(timeout, async {
            Manager::new()
                .await?
                .adapters()
                .await?
                .into_iter()
                .next()
                .ok_or(Error::NoAdapterAvailable)
        })
        .await?;
        Ok(Self {
            adapter,
            peripheral: None,
            state: State::new(permission),
            scanning: false,
            discovered: BTreeSet::new(),
            timeout,
            runtime,
            background: None,
            in_background: false,
        })
    }

    /// Subscribe before scanning/connecting; events are native asynchronous events.
    /// Consume continuously and call `observe` on disconnect/service/radio changes.
    pub async fn events(&self) -> Result<BoxStream<'static, CentralEvent>> {
        deadline(self.timeout, self.adapter.events()).await
    }

    /// Android ScanCallback failure codes (including registration/resource/rate limits).
    /// Subscribe before start_scan and consume alongside events. iOS reports radio
    /// state through events instead. One scan owner per adapter remains required.
    pub fn scan_errors(&self) -> BoxStream<'static, i32> {
        #[cfg(target_os = "android")]
        {
            Box::pin(android::scan_errors())
        }
        #[cfg(not(target_os = "android"))]
        {
            convenience::absent_scan_errors()
        }
    }

    /// Query the actual radio; btleplug 0.13.5 reports only Unknown on Android.
    pub async fn adapter_state(&self) -> Result<CentralState> {
        #[cfg(target_os = "android")]
        {
            android::adapter_state()
        }
        #[cfg(not(target_os = "android"))]
        {
            deadline(self.timeout, self.adapter.adapter_state()).await
        }
    }

    async fn ready(&self) -> Result<()> {
        self.state.ready()?;
        self.check_background()?;
        if self.adapter_state().await? != CentralState::PoweredOn {
            return Err(Error::NotSupported(
                "Bluetooth radio is not powered on".into(),
            ));
        }
        Ok(())
    }

    /// Invalidate cached capabilities when the OS reports disconnect or changes.
    pub async fn observe(&mut self, event: &CentralEvent) -> Result<()> {
        let matches = |id: &PeripheralId| self.peripheral.as_ref().is_some_and(|p| p.id() == *id);
        let invalidated = match event {
            CentralEvent::DeviceDisconnected(id) if matches(id) => {
                // A queued event can belong to the previous connection to this ID.
                let connected = deadline(
                    self.timeout,
                    self.peripheral.as_ref().unwrap().is_connected(),
                )
                .await;
                return self
                    .state
                    .reconcile_connection(connected, &mut self.discovered);
            }
            CentralEvent::DeviceServicesModified(id) if matches(id) => true,
            CentralEvent::StateUpdate(_) => self.adapter_state().await? != CentralState::PoweredOn,
            _ => false,
        };
        if invalidated {
            self.discovered.clear();
            self.state.recovery_needed = true;
        }
        Ok(())
    }

    pub async fn start_scan(&mut self, filter: ScanFilter) -> Result<()> {
        self.ready().await?;
        if self.in_background { return Err(Error::NotSupported("Scanning requires foreground use".into())); }
        if self.scanning {
            return Err(Error::NotSupported("stop the existing scan before starting another".into()));
        }
        // Mark before awaiting: cancellation leaves cleanup necessary.
        self.scanning = true;
        deadline(self.timeout, self.adapter.start_scan(filter)).await
    }

    pub async fn stop_scan(&mut self) -> Result<()> {
        if self.scanning {
            deadline(self.timeout, self.adapter.stop_scan()).await?;
            self.scanning = false;
        }
        Ok(())
    }

    pub async fn properties(&self, id: &PeripheralId) -> Result<Option<PeripheralProperties>> {
        self.ready().await?;
        deadline(self.timeout, async {
            self.adapter.peripheral(id).await?.properties().await
        })
        .await
    }

    /// Connect is separate from discovery. A timeout requires explicit disconnect
    /// or reconnect; dropping a future alone does not cancel native GATT requests.
    pub async fn connect(&mut self, id: &PeripheralId) -> Result<()> {
        self.state.admit_connection(self.in_background)?;
        self.ready().await?;
        if self.peripheral.is_some() {
            return Err(Error::NotSupported(
                "disconnect the current peripheral first".into(),
            ));
        }
        let peripheral = deadline(self.timeout, self.adapter.peripheral(id)).await?;
        self.peripheral = Some(peripheral);
        self.connect_current().await
    }

    async fn connect_current(&mut self) -> Result<()> {
        self.discovered.clear();
        self.state
            .operation(
                self.timeout,
                self.peripheral
                    .as_ref()
                    .ok_or(Error::NotConnected)?
                    .connect(),
            )
            .await?;
        Ok(())
    }

    pub async fn discover(&mut self) -> Result<BTreeSet<Service>> {
        self.ready().await?;
        let p = self.peripheral.as_ref().ok_or(Error::NotConnected)?;
        self.discovered.clear();
        self.state
            .operation(self.timeout, p.discover_services())
            .await?;
        self.discovered = p.characteristics();
        Ok(p.services())
    }

    async fn characteristic(
        &self,
        c: &Characteristic,
        flags: CharPropFlags,
    ) -> Result<&Peripheral> {
        self.ready().await?;
        state::validate_characteristic(&self.discovered, c, flags)?;
        let p = self.peripheral.as_ref().ok_or(Error::NotConnected)?;
        if !deadline(self.timeout, p.is_connected()).await? {
            return Err(Error::NotConnected);
        }
        Ok(p)
    }

    pub async fn read(&mut self, c: &Characteristic) -> Result<Vec<u8>> {
        let p = self.characteristic(c, CharPropFlags::READ).await?.clone();
        self.state.operation(self.timeout, p.read(c)).await
    }

    /// WithoutResponse means locally queued, not acknowledged by the peripheral.
    pub async fn write(&mut self, c: &Characteristic, data: &[u8], kind: WriteType) -> Result<()> {
        let flags = match kind {
            WriteType::WithResponse => CharPropFlags::WRITE,
            WriteType::WithoutResponse => CharPropFlags::WRITE_WITHOUT_RESPONSE,
        };
        let p = self.characteristic(c, flags).await?.clone();
        self.state
            .operation(self.timeout, p.write(c, data, kind))
            .await
    }

    /// Obtain the stream BEFORE `subscribe` to avoid missing the first value.
    /// After reconnect obtain a new stream and explicitly re-subscribe.
    pub async fn notifications(&self) -> Result<BoxStream<'static, ValueNotification>> {
        self.ready().await?;
        deadline(
            self.timeout,
            self.peripheral
                .as_ref()
                .ok_or(Error::NotConnected)?
                .notifications(),
        )
        .await
    }

    pub async fn subscribe(&mut self, c: &Characteristic) -> Result<()> {
        self.set_subscription(c, true).await
    }

    pub async fn unsubscribe(&mut self, c: &Characteristic) -> Result<()> {
        self.set_subscription(c, false).await
    }

    async fn set_subscription(&mut self, c: &Characteristic, enabled: bool) -> Result<()> {
        let p = self
            .characteristic(c, CharPropFlags::NOTIFY | CharPropFlags::INDICATE)
            .await?
            .clone();
        if enabled {
            self.state.operation(self.timeout, p.subscribe(c)).await?;
        } else {
            self.state.operation(self.timeout, p.unsubscribe(c)).await?;
        }
        Ok(())
    }

    /// Cleanup remains available while denied, paused, or recovering.
    pub async fn disconnect(&mut self) -> Result<()> {
        self.discovered.clear();
        self.state.recovery_needed = true;
        if let Some(p) = &self.peripheral {
            deadline(self.timeout, p.disconnect()).await?;
        }
        self.peripheral = None;
        self.state.recovery_needed = false;
        if let Some(lease) = &mut self.background { lease.release()?; }
        self.background.take();
        Ok(())
    }

    /// One explicit retry, never an unbounded background retry loop. Always
    /// disconnects first and re-discovers, so stale handles are not reused.
    /// The caller retains the target across retrieval errors or cancellation,
    /// even after this session's old peripheral handle has been cleared.
    pub async fn reconnect(&mut self, id: &PeripheralId) -> Result<BTreeSet<Service>> {
        self.state.admit_connection(self.in_background)?;
        self.disconnect().await?;
        #[cfg(target_vendor = "apple")]
        {
            // CoreBluetooth removes both cached objects on disconnect. Rehydrate
            // the native peripheral before looking it up or sending connect.
            let found = deadline(
                self.timeout,
                self.adapter
                    .retrieve_peripherals(btleplug::api::RetrievePeripheralsOptions {
                        identifiers: Some(vec![id.clone()]),
                        services: None,
                    }),
            )
            .await?;
            self.peripheral = Some(
                found
                    .into_iter()
                    .find(|p| p.id() == *id)
                    .ok_or(Error::DeviceNotFound)?,
            );
        }
        #[cfg(target_os = "android")]
        {
            self.peripheral = Some(deadline(self.timeout, self.adapter.add_peripheral(id)).await?);
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "android")))]
        {
            self.peripheral = Some(deadline(self.timeout, self.adapter.peripheral(id)).await?);
        }
        self.connect_current().await?;
        self.discover().await
    }

    /// Default: stop scanning AND disconnect. Explicit background opt-in preserves
    /// an uninterrupted connection only while its native context remains valid.
    pub async fn pause(&mut self) -> Vec<Error> {
        self.in_background = true;
        let mut context_error = None;
        if self.background.is_some() && self.state.ready().is_ok() {
            match self.check_background() {
                Ok(()) => return self.stop_scan().await.err().into_iter().collect(),
                Err(error) => context_error = Some(error),
            }
        }
        self.state.paused = true;
        state::cleanup_with_context(context_error, self.cleanup()).await
    }

    async fn cleanup(&mut self) -> Vec<Error> {
        let mut errors = Vec::new();
        if let Err(e) = self.stop_scan().await {
            errors.push(e);
        }
        if let Err(e) = self.disconnect().await {
            errors.push(e);
        }
        errors
    }

    /// Feed each native permission result/revocation here. Denial gates new work
    /// immediately and attempts cleanup; granting never auto-restarts a scan.
    pub async fn permission_changed(&mut self, permission: Permission) -> Vec<Error> {
        self.state.permission = permission;
        if permission != Permission::Granted {
            self.cleanup().await
        } else {
            Vec::new()
        }
    }

    /// Refresh permission on foreground entry. Explicitly restart scan/connect.
    pub fn resume(&mut self, permission: Permission) -> Result<()> {
        self.state.permission = permission;
        self.state.paused = false;
        self.in_background = false;
        self.state.ready()
    }

    pub async fn close(&mut self) -> Vec<Error> {
        self.state.paused = true;
        let mut errors = self.cleanup().await;
        // Stop the notification/service even if native disconnect failed; errors
        // remain observable and the connection handle stays available for retry.
        if let Some(lease) = &mut self.background {
            if let Err(error) = lease.release() { errors.push(error); }
        }
        self.background.take();
        errors
    }

    /// Opt in while the app is visible and already connected. Requires native
    /// host configuration; Android waits for actual foreground-service startup.
    /// No process-death restoration, background scanning, or implicit reconnect.
    pub async fn enable_background_connected(&mut self) -> Result<()> {
        self.ready().await?;
        if self.in_background { return Err(Error::NotSupported("Enable background BLE while visible".into())); }
        let p = self.peripheral.as_ref().ok_or(Error::NotConnected)?;
        if !deadline(self.timeout, p.is_connected()).await? { return Err(Error::NotConnected); }
        if self.background.is_none() { self.background = Some(background::Lease::acquire().await?); }
        Ok(())
    }

    pub fn check_background(&self) -> Result<()> {
        if let Some(lease) = &self.background { lease.check()?; }
        Ok(())
    }
}

async fn deadline<T>(duration: Duration, future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(duration, future)
        .await
        .map_err(|_| Error::TimedOut(duration))?
}

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-contract regression: cancellation cannot consume the caller's ID.
    // The ignored hardware test exercises the post-disconnect native state.
    #[test]
    fn reconnect_contract_retains_borrowed_target_for_retry() {
        fn retry_after_cancel<'a>(
            ble: &'a mut Ble,
            id: &'a PeripheralId,
        ) -> impl Future<Output = Result<BTreeSet<Service>>> + 'a {
            drop(ble.reconnect(id));
            ble.reconnect(id)
        }
        let _ = retry_after_cancel;
    }

    #[test]
    fn pending_native_operation_times_out_without_blocking_executor() {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let result: Result<()> =
                    deadline(Duration::from_millis(1), futures::future::pending()).await;
                assert!(matches!(result, Err(Error::TimedOut(_))));
            });
    }

    #[test]
    fn native_denial_is_not_success() {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let result: Result<()> = deadline(Duration::from_secs(1), async {
                    Err(Error::PermissionDenied)
                })
                .await;
                assert!(matches!(result, Err(Error::PermissionDenied)));
            });
    }
}

#[cfg(test)]
#[path = "tests/hardware.rs"]
mod hardware;
