use super::{events, LocationAccuracy, LocationSettings, Position};
use std::ffi::{c_char, CStr};

#[link(name = "CoreLocation", kind = "framework")]
extern "C" {}
extern "C" {
    fn gpui_location_start(id: u64, accuracy: f64, distance: f64, once: bool, timeout_ms: u64, background: bool, previous: u64);
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
pub(super) fn start(id: u64, settings: &LocationSettings, once: bool, timeout_ms: u64, background: bool, previous: u64) -> Result<(), String> {
    let accuracy = match settings.accuracy {
        LocationAccuracy::Lowest => 3000.0,
        LocationAccuracy::Low => 1000.0,
        LocationAccuracy::Medium => 100.0,
        LocationAccuracy::High => 10.0,
        LocationAccuracy::Best => -1.0,
        LocationAccuracy::BestForNavigation => -2.0,
    };
    unsafe {
        gpui_location_start(id, accuracy, settings.distance_filter, once, timeout_ms, background, previous);
    }
    Ok(())
}
pub(super) fn stop(id: u64) {
    unsafe {
        gpui_location_stop(id);
    }
}

#[cfg(feature = "permission_handler")]
pub(super) fn request_permission(always: bool) -> Result<crate::packages::permission_handler::PermissionStatus, String> {
    use crate::packages::permission_handler::{self, Permission, PermissionStatus};
    use std::{ffi::c_void, sync::mpsc};
    struct Request {
        always: bool,
        sender: mpsc::Sender<Result<PermissionStatus, String>>,
    }
    unsafe extern "C" fn invoke(context: *mut c_void) {
        let request = Box::from_raw(context.cast::<Request>());
        let permission = if request.always { Permission::LocationAlways } else { Permission::LocationWhenInUse };
        let _ = request.sender.send(permission_handler::request_permission(permission));
    }
    extern "C" {
        fn gpui_location_on_main(context: *mut c_void, callback: unsafe extern "C" fn(*mut c_void));
    }
    let (sender, receiver) = mpsc::channel();
    let request = Box::into_raw(Box::new(Request { always, sender }));
    // Existing permission owner pumps UIKit's run loop while awaiting its delegate.
    // No mutex is held across dispatch/wait, and the callback owns the boxed request.
    unsafe { gpui_location_on_main(request.cast(), invoke); }
    receiver.recv().map_err(|_| "Location permission callback disconnected".to_string())?
}
/// Called synchronously by the owned native delegate with a valid UTF-8 string.
#[no_mangle]
unsafe extern "C" fn gpui_location_event(id: u64, raw: *const c_char) {
    if raw.is_null() {
        return;
    }
    let event = CStr::from_ptr(raw).to_string_lossy().into_owned();
    if !events::send(id, event) {
        stop(id);
    }
}
