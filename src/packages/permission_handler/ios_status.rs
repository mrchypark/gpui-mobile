use super::PermissionStatus;

pub(super) fn location(status: i32, always: bool) -> PermissionStatus {
    match status {
        1 => PermissionStatus::Restricted,
        2 => PermissionStatus::PermanentlyDenied,
        3 => PermissionStatus::Granted,
        4 if !always => PermissionStatus::Granted,
        _ => PermissionStatus::Denied,
    }
}

// MediaPlayer uses denied=1/restricted=2, unlike AVFoundation's ordering.
pub(super) fn media_library(status: i64) -> PermissionStatus {
    match status {
        0 => PermissionStatus::Denied,
        1 => PermissionStatus::PermanentlyDenied,
        2 => PermissionStatus::Restricted,
        3 => PermissionStatus::Granted,
        _ => PermissionStatus::Denied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn when_in_use_is_not_always_authorization() {
        assert_eq!(location(4, false), PermissionStatus::Granted);
        assert_eq!(location(4, true), PermissionStatus::Denied);
        assert_eq!(location(3, true), PermissionStatus::Granted);
        assert_eq!(location(0, true), PermissionStatus::Denied);
        assert_eq!(location(1, true), PermissionStatus::Restricted);
        assert_eq!(location(2, false), PermissionStatus::PermanentlyDenied);
    }
    #[test]
    fn media_library_native_values_do_not_use_avfoundation_order() {
        assert_eq!(media_library(0), PermissionStatus::Denied);
        assert_eq!(media_library(1), PermissionStatus::PermanentlyDenied);
        assert_eq!(media_library(2), PermissionStatus::Restricted);
        assert_eq!(media_library(3), PermissionStatus::Granted);
        assert_eq!(media_library(99), PermissionStatus::Denied);
    }
}
