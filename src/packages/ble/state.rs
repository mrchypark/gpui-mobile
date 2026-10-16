use super::{CharPropFlags, Characteristic, Error, Result};
use std::collections::BTreeSet;

/// Losing a requested background context is an error even when the fallback
/// stop/disconnect succeeds. Preserve it before any independent cleanup errors.
pub(super) async fn cleanup_with_context(
    context_error: Option<Error>,
    cleanup: impl std::future::Future<Output = Vec<Error>>,
) -> Vec<Error> {
    let mut errors: Vec<_> = context_error.into_iter().collect();
    errors.extend(cleanup.await);
    errors
}

#[derive(Default)]
pub(super) struct ScanErrors(std::sync::Mutex<Vec<futures::channel::mpsc::Sender<i32>>>);
impl ScanErrors {
    pub fn subscribe(&self) -> futures::channel::mpsc::Receiver<i32> {
        let (sender, receiver) = futures::channel::mpsc::channel(1);
        let mut senders = self.0.lock().unwrap();
        senders.retain(|sender| !sender.is_closed());
        senders.push(sender);
        receiver
    }
    pub fn send(&self, code: i32) {
        self.0.lock().unwrap().retain_mut(|sender| {
            // A full channel already contains a scan failure; never block native callbacks.
            sender
                .try_send(code)
                .map_or_else(|error| error.is_full(), |_| true)
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Unknown,
    Granted,
    Denied,
    Restricted,
}

pub(super) struct State {
    pub permission: Permission,
    pub paused: bool,
    pub recovery_needed: bool,
}

impl State {
    pub async fn operation<T>(
        &mut self,
        timeout: std::time::Duration,
        future: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        // Set before polling native work; dropping a pending future keeps recovery required.
        self.recovery_needed = true;
        let value = super::deadline(timeout, future).await?;
        self.recovery_needed = false;
        Ok(value)
    }
    pub fn reconcile_connection(
        &mut self,
        connected: Result<bool>,
        cache: &mut BTreeSet<Characteristic>,
    ) -> Result<()> {
        if !matches!(connected, Ok(true)) {
            cache.clear();
            self.recovery_needed = true;
        }
        connected.map(|_| ())
    }
    pub fn new(permission: Permission) -> Self {
        Self {
            permission,
            paused: false,
            recovery_needed: false,
        }
    }
    pub fn allowed(&self) -> Result<()> {
        if self.permission != Permission::Granted {
            return Err(Error::PermissionDenied);
        }
        if self.paused {
            return Err(Error::NotSupported("BLE session is paused".into()));
        }
        Ok(())
    }
    pub fn ready(&self) -> Result<()> {
        self.allowed()?;
        if self.recovery_needed {
            return Err(Error::NotSupported(
                "disconnect/reconnect required after interruption".into(),
            ));
        }
        Ok(())
    }
}

pub(super) fn validate_characteristic(
    discovered: &BTreeSet<Characteristic>,
    characteristic: &Characteristic,
    flags: CharPropFlags,
) -> Result<()> {
    if !discovered.contains(characteristic) {
        return Err(Error::NoSuchCharacteristic);
    }
    if !characteristic.properties.intersects(flags) {
        return Err(Error::NotSupported(
            "characteristic does not advertise this operation".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lost_background_context_survives_successful_and_failed_cleanup() {
        futures::executor::block_on(async {
            let ran = std::cell::Cell::new(false);
            let errors = cleanup_with_context(Some(Error::PermissionDenied), async {
                ran.set(true); Vec::new()
            }).await;
            assert!(ran.get());
            assert!(matches!(errors.as_slice(), [Error::PermissionDenied]));
            let errors = cleanup_with_context(Some(Error::PermissionDenied), async {
                vec![Error::NotConnected, Error::TimedOut(std::time::Duration::from_secs(1))]
            }).await;
            assert!(matches!(errors.as_slice(), [Error::PermissionDenied, Error::NotConnected, Error::TimedOut(_)]));
            assert!(cleanup_with_context(None, async { Vec::new() }).await.is_empty());
        });
    }
    #[test]
    fn interrupted_native_work_requires_recovery_even_after_permission_returns() {
        use futures::FutureExt;
        use std::time::Duration;
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let mut state = State::new(Permission::Granted);
                assert!(state
                    .operation::<()>(Duration::from_secs(1), futures::future::pending())
                    .now_or_never()
                    .is_none());
                assert!(state.ready().is_err());
                state.permission = Permission::Denied;
                state.permission = Permission::Granted;
                assert!(state.ready().is_err());
                assert!(matches!(
                    state
                        .operation::<()>(Duration::from_millis(1), futures::future::pending())
                        .await,
                    Err(Error::TimedOut(_))
                ));
                assert!(state.ready().is_err());
                assert!(state
                    .operation::<()>(Duration::from_secs(1), async {
                        Err(Error::PermissionDenied)
                    })
                    .await
                    .is_err());
                assert!(state.ready().is_err());
                state
                    .operation(Duration::from_secs(1), async { Ok(()) })
                    .await
                    .unwrap();
                assert!(state.ready().is_ok());
            });
    }
    #[test]
    fn native_scan_failure_wakes_subscribers_without_unbounded_backlog() {
        use futures::{FutureExt, StreamExt};
        let errors = ScanErrors::default();
        let mut first = errors.subscribe();
        let mut second = errors.subscribe();
        assert!(first.next().now_or_never().is_none());
        errors.send(3);
        assert_eq!(first.next().now_or_never(), Some(Some(3)));
        assert_eq!(second.next().now_or_never(), Some(Some(3)));
        drop(second);
        for _ in 0..1000 {
            errors.send(6);
        }
        assert_eq!(first.next().now_or_never(), Some(Some(6)));
        assert_eq!(errors.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn duplicate_uuid_membership_uses_service_identity() {
        let c = Characteristic {
            uuid: "00002a37-0000-1000-8000-00805f9b34fb".parse().unwrap(),
            service_uuid: "0000180d-0000-1000-8000-00805f9b34fb".parse().unwrap(),
            properties: CharPropFlags::READ | CharPropFlags::WRITE,
            descriptors: BTreeSet::new(),
        };
        let mut other = c.clone();
        other.service_uuid = "0000180f-0000-1000-8000-00805f9b34fb".parse().unwrap();
        let discovered = BTreeSet::from([c.clone(), other.clone()]);
        assert!(validate_characteristic(&discovered, &c, CharPropFlags::READ).is_ok());
        assert!(validate_characteristic(&discovered, &other, CharPropFlags::WRITE).is_ok());
        other.service_uuid = "00001810-0000-1000-8000-00805f9b34fb".parse().unwrap();
        assert!(validate_characteristic(&discovered, &other, CharPropFlags::READ).is_err());
    }
    #[test]
    fn queued_disconnect_after_reconnect_keeps_new_connection_valid() {
        let mut state = State::new(Permission::Granted);
        let mut cache = BTreeSet::new();
        state.reconcile_connection(Ok(false), &mut cache).unwrap();
        assert!(state.ready().is_err());
        // New connection succeeds, then the prior connection's event is consumed.
        state.recovery_needed = false;
        state.reconcile_connection(Ok(true), &mut cache).unwrap();
        assert!(state.ready().is_ok());
        // Failure to query the OS must not preserve permission to use stale GATT.
        assert!(state
            .reconcile_connection(Err(Error::PermissionDenied), &mut cache)
            .is_err());
        assert!(state.ready().is_err());
    }
    #[test]
    fn permission_recovery_does_not_override_pause_or_disconnect() {
        let mut state = State::new(Permission::Unknown);
        for denied in [
            Permission::Unknown,
            Permission::Denied,
            Permission::Restricted,
        ] {
            state.permission = denied;
            assert!(matches!(state.ready(), Err(Error::PermissionDenied)));
        }
        state.permission = Permission::Granted;
        state.paused = true;
        assert!(state.ready().is_err());
        state.paused = false;
        state.recovery_needed = true;
        assert!(state.ready().is_err());
        assert!(state.allowed().is_ok()); // recovery itself remains permitted
        state.recovery_needed = false;
        assert!(state.ready().is_ok());
    }

    #[test]
    fn stale_handles_and_unsupported_writes_are_rejected() {
        let c = Characteristic {
            uuid: "00002a37-0000-1000-8000-00805f9b34fb".parse().unwrap(),
            service_uuid: "0000180d-0000-1000-8000-00805f9b34fb".parse().unwrap(),
            properties: CharPropFlags::READ | CharPropFlags::INDICATE,
            descriptors: BTreeSet::new(),
        };
        let mut known = BTreeSet::from([c.clone()]);
        assert!(validate_characteristic(&known, &c, CharPropFlags::READ).is_ok());
        assert!(validate_characteristic(
            &known,
            &c,
            CharPropFlags::NOTIFY | CharPropFlags::INDICATE
        )
        .is_ok());
        assert!(matches!(
            validate_characteristic(&known, &c, CharPropFlags::WRITE),
            Err(Error::NotSupported(_))
        ));
        known.clear(); // disconnect, service change, or reconnect
        assert!(matches!(
            validate_characteristic(&known, &c, CharPropFlags::READ),
            Err(Error::NoSuchCharacteristic)
        ));
    }
}
