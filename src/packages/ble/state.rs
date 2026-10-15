use super::{CharPropFlags, Characteristic, Error, Result};
use std::collections::BTreeSet;

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
