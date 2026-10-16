//! Network connectivity status.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

/// Current network connectivity status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectivityStatus {
    /// Connected via WiFi.
    Wifi,
    /// Connected via cellular (mobile data).
    Cellular,
    /// No observed WiFi/cellular connection, unavailable status, or an unrepresented transport.
    None,
}

/// Check the current network connectivity status.
/// This describes the local network path, not verified Internet access.
/// iOS waits at most one second for a native path snapshot (iOS 12+).
pub fn check_connectivity() -> ConnectivityStatus {
    #[cfg(target_os = "ios")]
    {
        ios::check_connectivity()
    }
    #[cfg(target_os = "android")]
    {
        android::check_connectivity()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        ConnectivityStatus::None
    }
}

#[cfg(any(target_os = "ios", test))]
fn classify_path(connected: bool, wifi: bool, cellular: bool) -> ConnectivityStatus {
    if !connected {
        ConnectivityStatus::None
    } else if wifi {
        ConnectivityStatus::Wifi
    } else if cellular {
        ConnectivityStatus::Cellular
    } else {
        ConnectivityStatus::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrepresented_transport_is_not_wifi() {
        assert_eq!(classify_path(true, false, false), ConnectivityStatus::None);
        assert_eq!(classify_path(false, true, false), ConnectivityStatus::None);
        assert_eq!(classify_path(true, true, false), ConnectivityStatus::Wifi);
        assert_eq!(
            classify_path(true, false, true),
            ConnectivityStatus::Cellular
        );
    }
}
