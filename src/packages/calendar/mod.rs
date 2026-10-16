//! Calendar access for reading and writing device calendar events.
//!
//! Provides a cross-platform calendar API backed by:
//! - Android: CalendarContract via JNI
//! - iOS: EventKit (EKEventStore) via Objective-C
//!
//! Feature-gated behind `calendar`.

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", test))]
mod authorization;
#[cfg(target_os = "ios")]
mod ios;

/// A calendar on the device.
#[derive(Debug, Clone)]
pub struct Calendar {
    /// Platform-specific calendar identifier.
    pub id: String,
    /// Calendar display name.
    pub name: String,
    /// Whether this calendar is read-only.
    pub is_read_only: bool,
    /// Calendar color as ARGB hex (e.g. 0xFF4285F4).
    pub color: u32,
}

/// A calendar event.
#[derive(Debug, Clone)]
pub struct CalendarEvent {
    /// Platform-specific event identifier (empty for new events).
    pub id: String,
    /// Event title.
    pub title: String,
    /// Event description/notes.
    pub description: String,
    /// Event location.
    pub location: String,
    /// Start time as Unix timestamp in milliseconds.
    pub start_ms: i64,
    /// End time as Unix timestamp in milliseconds.
    pub end_ms: i64,
    /// Whether this is an all-day event. Creation requires UTC-midnight start
    /// and end timestamps; end is exclusive. No local-time normalization occurs.
    pub all_day: bool,
    /// Calendar ID this event belongs to.
    pub calendar_id: String,
}

/// Get all calendars on the device.
pub fn get_calendars() -> Result<Vec<Calendar>, String> {
    #[cfg(target_os = "ios")]
    {
        ios::get_calendars()
    }
    #[cfg(target_os = "android")]
    {
        android::get_calendars()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        Err("calendar is only available on iOS and Android".into())
    }
}

/// Get events from a date range.
/// `start_ms` and `end_ms` are Unix timestamps in milliseconds.
pub fn get_events(
    calendar_id: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<CalendarEvent>, String> {
    if end_ms <= start_ms {
        return Err("Calendar range end must follow its start".into());
    }
    #[cfg(target_os = "ios")]
    {
        ios::get_events(calendar_id, start_ms, end_ms)
    }
    #[cfg(target_os = "android")]
    {
        android::get_events(calendar_id, start_ms, end_ms)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = (calendar_id, start_ms, end_ms);
        Err("calendar is only available on iOS and Android".into())
    }
}

/// Create a new event. Returns the event ID. iOS requires full calendar access
/// because the selected calendar is looked up by ID (write-only is insufficient).
pub fn create_event(event: &CalendarEvent) -> Result<String, String> {
    validate_event_times(event.start_ms, event.end_ms, event.all_day)?;
    #[cfg(target_os = "ios")]
    {
        ios::create_event(event)
    }
    #[cfg(target_os = "android")]
    {
        android::create_event(event)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = event;
        Err("calendar is only available on iOS and Android".into())
    }
}

/// Delete a non-recurring event by ID. Recurring series/occurrences are rejected
/// because this API has no deletion-scope argument. Never deletes a whole series.
pub fn delete_event(event_id: &str) -> Result<bool, String> {
    #[cfg(target_os = "ios")]
    {
        ios::delete_event(event_id)
    }
    #[cfg(target_os = "android")]
    {
        android::delete_event(event_id)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = event_id;
        Err("calendar is only available on iOS and Android".into())
    }
}

fn validate_event_times(start: i64, end: i64, all_day: bool) -> Result<(), String> {
    if end <= start {
        return Err("Calendar event end must follow its start".into());
    }
    if all_day && (start.rem_euclid(86_400_000) != 0 || end.rem_euclid(86_400_000) != 0) {
        return Err(
            "All-day events require UTC-midnight timestamps and an exclusive end day".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_event_times;
    #[test]
    fn all_day_boundaries_are_explicit_not_silently_shifted() {
        assert!(validate_event_times(0, 86_400_000, true).is_ok());
        assert!(validate_event_times(-86_400_000, 0, true).is_ok());
        assert!(validate_event_times(1, 86_400_000, true).is_err());
        assert!(validate_event_times(0, 86_400_001, true).is_err());
        assert!(validate_event_times(0, 0, true).is_err());
        assert!(validate_event_times(i64::MIN, i64::MAX, false).is_ok());
        assert!(validate_event_times(1, 2, false).is_ok());
    }
}
