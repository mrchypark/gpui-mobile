use std::ffi::{c_char, CStr, CString};

extern "C" {
    fn gpui_media_init(error: *mut c_char, capacity: usize) -> bool;
    fn gpui_media_metadata(
        title: *const c_char,
        artist: *const c_char,
        duration_ms: u64,
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
    fn gpui_media_playback(
        playing: bool,
        position_ms: u64,
        speed: f32,
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
    fn gpui_media_release(error: *mut c_char, capacity: usize) -> bool;
}
fn native(call: impl FnOnce(*mut c_char, usize) -> bool) -> Result<(), String> {
    let mut error = [0 as c_char; 512];
    if call(error.as_mut_ptr(), error.len()) {
        Ok(())
    } else {
        Err(unsafe { CStr::from_ptr(error.as_ptr()) }
            .to_string_lossy()
            .into_owned())
    }
}
pub fn init() -> Result<(), String> {
    native(|error, capacity| unsafe { gpui_media_init(error, capacity) })
}
pub fn set_metadata(title: &str, artist: &str, duration_ms: u64) -> Result<(), String> {
    let title = CString::new(title).map_err(|e| e.to_string())?;
    let artist = CString::new(artist).map_err(|e| e.to_string())?;
    native(|error, capacity| unsafe {
        gpui_media_metadata(
            title.as_ptr(),
            artist.as_ptr(),
            duration_ms,
            error,
            capacity,
        )
    })
}
pub fn set_playback_state(playing: bool, position_ms: u64, speed: f32) -> Result<(), String> {
    native(|error, capacity| unsafe {
        gpui_media_playback(playing, position_ms, speed, error, capacity)
    })
}
pub fn release() -> Result<(), String> {
    native(|error, capacity| unsafe { gpui_media_release(error, capacity) })
}

#[no_mangle]
extern "C" fn gpui_media_action(action: u32) -> bool {
    let action = match action {
        0 => super::MediaAction::Play,
        1 => super::MediaAction::Pause,
        2 => super::MediaAction::Stop,
        3 => super::MediaAction::Next,
        4 => super::MediaAction::Previous,
        _ => return false,
    };
    std::panic::catch_unwind(|| super::notify_action(action)).unwrap_or(false)
}
#[no_mangle]
extern "C" fn gpui_media_seek(seconds: f64) -> bool {
    if !seconds.is_finite() || seconds < 0.0 || seconds > u64::MAX as f64 / 1000.0 {
        return false;
    }
    std::panic::catch_unwind(|| super::notify_seek((seconds * 1000.0).round() as u64))
        .unwrap_or(false)
}
