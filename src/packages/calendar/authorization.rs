// EventKit's legacy Authorized and iOS17 FullAccess both have value 3.
// WriteOnly (4) cannot read/identify the calendar selected by this API.
pub(super) fn require_read(status: i64) -> Result<(), String> {
    if status == 3 {
        Ok(())
    } else {
        Err("Calendar full read access is required; request permission separately".into())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn read_access_does_not_confuse_write_only_or_unknown_with_empty() {
        assert!(super::require_read(3).is_ok());
        for status in [0, 1, 2, 4, 5, -1] {
            assert!(super::require_read(status).is_err());
        }
    }
}
