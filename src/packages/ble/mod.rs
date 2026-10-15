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
pub use state::Permission;
use state::State;

/// One central session and one active peripheral. Create separate sessions only
/// if the application deliberately coordinates adapter-wide scan ownership.
/// Call `pause`/`close` before dropping; Drop cannot await OS cleanup.
pub struct Ble {
    adapter: Adapter,
    peripheral: Option<Peripheral>,
    state: State,
    scanning: bool,
    discovered: BTreeSet<Characteristic>,
    timeout: Duration,
}

impl Ble {
    /// Permissions are supplied by the application's native permission callback.
    /// `Granted` is not a substitute for OS permission; native errors propagate.
    pub async fn new(permission: Permission, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::NotSupported("BLE timeout must be nonzero".into()));
        }
        tokio::runtime::Handle::try_current()
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
        })
    }

    /// Subscribe before scanning/connecting; events are native asynchronous events.
    /// Consume continuously and call `observe` on disconnect/service/radio changes.
    pub async fn events(&self) -> Result<BoxStream<'static, CentralEvent>> {
        deadline(self.timeout, self.adapter.events()).await
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
            CentralEvent::StateUpdate(_) => {
                deadline(self.timeout, self.adapter.adapter_state()).await?
                    != CentralState::PoweredOn
            }
            _ => false,
        };
        if invalidated {
            self.discovered.clear();
            self.state.recovery_needed = true;
        }
        Ok(())
    }

    pub async fn start_scan(&mut self, filter: ScanFilter) -> Result<()> {
        self.state.ready()?;
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
        self.state.ready()?;
        deadline(self.timeout, async {
            self.adapter.peripheral(id).await?.properties().await
        })
        .await
    }

    /// Connect is separate from discovery. A timeout requires explicit disconnect
    /// or reconnect; dropping a future alone does not cancel native GATT requests.
    pub async fn connect(&mut self, id: &PeripheralId) -> Result<()> {
        self.state.ready()?;
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
        self.state.recovery_needed = true;
        deadline(
            self.timeout,
            self.peripheral
                .as_ref()
                .ok_or(Error::NotConnected)?
                .connect(),
        )
        .await?;
        self.state.recovery_needed = false;
        Ok(())
    }

    pub async fn discover(&mut self) -> Result<BTreeSet<Service>> {
        self.state.ready()?;
        let p = self.peripheral.as_ref().ok_or(Error::NotConnected)?;
        self.discovered.clear();
        self.state.recovery_needed = true;
        deadline(self.timeout, p.discover_services()).await?;
        self.discovered = p.characteristics();
        self.state.recovery_needed = false;
        Ok(p.services())
    }

    async fn characteristic(
        &self,
        c: &Characteristic,
        flags: CharPropFlags,
    ) -> Result<&Peripheral> {
        self.state.ready()?;
        state::validate_characteristic(&self.discovered, c, flags)?;
        let p = self.peripheral.as_ref().ok_or(Error::NotConnected)?;
        if !deadline(self.timeout, p.is_connected()).await? {
            return Err(Error::NotConnected);
        }
        Ok(p)
    }

    pub async fn read(&mut self, c: &Characteristic) -> Result<Vec<u8>> {
        let p = self.characteristic(c, CharPropFlags::READ).await?.clone();
        self.state.recovery_needed = true;
        let value = deadline(self.timeout, p.read(c)).await?;
        self.state.recovery_needed = false;
        Ok(value)
    }

    /// WithoutResponse means locally queued, not acknowledged by the peripheral.
    pub async fn write(&mut self, c: &Characteristic, data: &[u8], kind: WriteType) -> Result<()> {
        let flags = match kind {
            WriteType::WithResponse => CharPropFlags::WRITE,
            WriteType::WithoutResponse => CharPropFlags::WRITE_WITHOUT_RESPONSE,
        };
        let p = self.characteristic(c, flags).await?.clone();
        self.state.recovery_needed = true;
        deadline(self.timeout, p.write(c, data, kind)).await?;
        self.state.recovery_needed = false;
        Ok(())
    }

    /// Obtain the stream BEFORE `subscribe` to avoid missing the first value.
    /// After reconnect obtain a new stream and explicitly re-subscribe.
    pub async fn notifications(&self) -> Result<BoxStream<'static, ValueNotification>> {
        self.state.ready()?;
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
        self.state.recovery_needed = true;
        if enabled {
            deadline(self.timeout, p.subscribe(c)).await?;
        } else {
            deadline(self.timeout, p.unsubscribe(c)).await?;
        }
        self.state.recovery_needed = false;
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
        Ok(())
    }

    /// One explicit retry, never an unbounded background retry loop. Always
    /// disconnects first and re-discovers, so stale handles are not reused.
    /// The caller retains the target across retrieval errors or cancellation,
    /// even after this session's old peripheral handle has been cleared.
    pub async fn reconnect(&mut self, id: &PeripheralId) -> Result<BTreeSet<Service>> {
        self.state.allowed()?;
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

    /// Foreground-only policy: stop scanning AND disconnect, attempting both even
    /// if one fails. No operation is silently reported as cleaned up on failure.
    pub async fn pause(&mut self) -> Vec<Error> {
        self.state.paused = true;
        self.cleanup().await
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
        self.state.ready()
    }

    pub async fn close(&mut self) -> Vec<Error> {
        self.pause().await
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
