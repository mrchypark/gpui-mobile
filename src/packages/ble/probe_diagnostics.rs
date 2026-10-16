// Debug fixture classification: never return native message text or coordinates.
pub(super) fn location_error(message: &str) -> &'static str {
    match message.split_once(':').map(|(code, _)| code) {
        Some("timeout") => "timeout",
        Some("permission_denied") => "permission_denied",
        Some("disabled") => "disabled",
        Some("unavailable") => "unavailable",
        Some("configuration") => "configuration",
        Some("native") => "native",
        Some("overflow") => "overflow",
        _ if message == "Location permission denied" => "permission_denied",
        _ if message.starts_with("Location subscription ended") => "ended",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_errors_are_whitelisted_without_payload() {
        for code in ["timeout", "permission_denied", "disabled", "unavailable", "configuration", "native", "overflow"] {
            assert_eq!(location_error(&format!("{code}: PRIVATE_NATIVE_PAYLOAD")), code);
        }
        assert_eq!(location_error("Location permission denied"), "permission_denied");
        assert_eq!(location_error("Location subscription ended: PRIVATE"), "ended");
        assert_eq!(location_error("37.123, 127.456"), "other");
        assert_eq!(location_error("unknown: PRIVATE"), "other");
    }
}
