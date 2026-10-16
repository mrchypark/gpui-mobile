//! Reuses the existing permission owner; never creates a second permission delegate.
use crate::packages::permission_handler::{self, Permission, PermissionStatus};

/// Permission and service state are independent: a grant does not enable GPS.
#[derive(Debug, Clone, Copy)]
pub struct LocationAccess {
    pub service_enabled: bool,
    pub permission: PermissionStatus,
}

impl LocationAccess {
    pub fn can_request_position(self) -> bool {
        self.service_enabled && self.permission.is_granted()
    }
    pub fn guidance(self) -> &'static str {
        if !self.service_enabled {
            return "Enable Location Services in system Settings, then refresh the watch.";
        }
        match self.permission {
            PermissionStatus::Denied => "Request location permission from an explicit user action.",
            PermissionStatus::PermanentlyDenied => "Open app Settings to allow location, then refresh the watch.",
            PermissionStatus::Restricted => "Location access is restricted by system policy.",
            _ => "Location access is available; watch permission events report approximate or precise access. Requested accuracy is not guaranteed.",
        }
    }
}

/// Read-only preflight. Does not prompt or open Settings.
pub fn check_location_access() -> Result<LocationAccess, String> {
    Ok(LocationAccess {
        service_enabled: super::is_location_service_enabled()?,
        permission: permission_handler::check_permission(Permission::LocationWhenInUse)?,
    })
}

/// Explicit user action; call on a worker, following permission_handler's thread contract.
/// Returns the existing permission backend's result without retrying or masking errors.
pub fn request_location_access() -> Result<LocationAccess, String> {
    let permission = request_permission(false)?;
    Ok(LocationAccess {
        service_enabled: super::is_location_service_enabled()?,
        permission,
    })
}

pub(super) fn request_permission(always: bool) -> Result<PermissionStatus, String> {
    #[cfg(target_os = "ios")]
    { super::ios::request_permission(always) }
    #[cfg(not(target_os = "ios"))]
    { permission_handler::request_permission(if always { Permission::LocationAlways } else { Permission::LocationWhenInUse }) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grant_does_not_override_disabled_service_or_restriction() {
        for permission in [PermissionStatus::Granted, PermissionStatus::Denied,
            PermissionStatus::Restricted, PermissionStatus::PermanentlyDenied] {
            let access = LocationAccess { service_enabled: false, permission };
            assert!(!access.can_request_position());
            assert!(access.guidance().contains("Enable Location Services"));
        }
        assert!(!LocationAccess { service_enabled: true, permission: PermissionStatus::Restricted }.can_request_position());
        assert!(LocationAccess { service_enabled: true, permission: PermissionStatus::Granted }.can_request_position());
    }
}
