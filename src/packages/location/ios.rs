use super::{events, LocationAccuracy, LocationSettings, Position};
use std::ffi::{c_char, CStr};

#[link(name = "CoreLocation", kind = "framework")]
extern "C" {}
extern "C" {
    fn gpui_location_start(id: u64, accuracy: f64, distance: f64, once: bool);
    fn gpui_location_stop(id: u64);
    fn gpui_location_enabled() -> bool;
    fn gpui_location_is_main_thread() -> bool;
    fn gpui_location_last(buffer: *mut c_char, capacity: usize) -> bool;
}
pub fn is_location_service_enabled() -> Result<bool, String> {
    Ok(unsafe { gpui_location_enabled() })
}
pub fn get_current_position(settings: &LocationSettings) -> Result<Position, String> {
    if unsafe { gpui_location_is_main_thread() } {
        return Err("Use get_current_position_async on the UI thread".into());
    }
    futures::executor::block_on(super::get_current_position_async(settings))
}
pub fn get_last_known_position() -> Result<Option<Position>, String> {
    let mut buffer = [0 as c_char; 512];
    if !unsafe { gpui_location_last(buffer.as_mut_ptr(), buffer.len()) } {
        return Ok(None);
    }
    let raw = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_str()
        .map_err(|e| e.to_string())?;
    super::stream::parse_position(raw).map(Some)
}
pub(super) fn start(id: u64, settings: &LocationSettings, once: bool) -> Result<(), String> {
    let accuracy = match settings.accuracy {
        LocationAccuracy::Lowest => 3000.0,
        LocationAccuracy::Low => 1000.0,
        LocationAccuracy::Medium => 100.0,
        LocationAccuracy::High => 10.0,
        LocationAccuracy::Best => -1.0,
        LocationAccuracy::BestForNavigation => -2.0,
    };
    unsafe {
        gpui_location_start(id, accuracy, settings.distance_filter, once);
    }
    Ok(())
}
pub(super) fn stop(id: u64) {
    unsafe {
        gpui_location_stop(id);
    }
}
/// Called synchronously by the owned native delegate with a valid UTF-8 string.
#[no_mangle]
unsafe extern "C" fn gpui_location_event(id: u64, raw: *const c_char) {
    if raw.is_null() {
        return;
    }
    let event = CStr::from_ptr(raw).to_string_lossy().into_owned();
    events::send(id, event);
}
