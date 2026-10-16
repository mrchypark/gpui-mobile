//! System media session integration for Android and iOS.
//!
//! Provides:
//! - Media notification with playback controls (Android notification shade, iOS lock screen)
//! - System panel playback info (title, artist, duration, progress)
//! - Media button handling (headphone buttons, car controls)
//!
//! Feature-gated behind `media_session`.
//!
//! On iOS this owns the shared Now Playing information and configures the shared
//! audio session for playback until `release`. Audio playback itself belongs to
//! the caller; background playback additionally requires the host's audio mode.
//! Remote handlers run synchronously on the native callback thread and should
//! return promptly. A handler may replace handlers or release the session.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

use std::sync::{Arc, Mutex};

/// Callback for media actions from system controls.
pub type MediaActionCallback = Box<dyn Fn(MediaAction) + Send + 'static>;

/// Actions that can be triggered from system media controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaAction {
    Play,
    Pause,
    Stop,
    Next,
    Previous,
}

/// Callback for seek requests from system controls.
pub type MediaSeekCallback = Box<dyn Fn(u64) + Send + 'static>;

static ACTION_CALLBACK: Mutex<Option<Arc<Mutex<MediaActionCallback>>>> = Mutex::new(None);
static SEEK_CALLBACK: Mutex<Option<Arc<Mutex<MediaSeekCallback>>>> = Mutex::new(None);

/// Register a callback for media actions from system controls.
pub fn set_action_handler(handler: impl Fn(MediaAction) + Send + 'static) {
    *ACTION_CALLBACK.lock().unwrap() = Some(Arc::new(Mutex::new(Box::new(handler))));
}

/// Register a callback for seek requests from system controls.
pub fn set_seek_handler(handler: impl Fn(u64) + Send + 'static) {
    *SEEK_CALLBACK.lock().unwrap() = Some(Arc::new(Mutex::new(Box::new(handler))));
}

/// Called from platform code when a media action is received.
#[allow(dead_code)]
pub(crate) fn notify_action(action: MediaAction) -> bool {
    let callback = ACTION_CALLBACK.lock().unwrap().clone();
    if let Some(cb) = callback {
        cb.lock().unwrap()(action);
        true
    } else {
        false
    }
}

/// Called from platform code when a seek request is received.
#[allow(dead_code)]
pub(crate) fn notify_seek(position_ms: u64) -> bool {
    let callback = SEEK_CALLBACK.lock().unwrap().clone();
    if let Some(cb) = callback {
        cb.lock().unwrap()(position_ms);
        true
    } else {
        false
    }
}

/// Initialize the media session. Call once before using other functions.
pub fn init() -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        android::init()
    }
    #[cfg(target_os = "ios")]
    {
        ios::init()
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        Err("media_session is only available on iOS and Android".into())
    }
}

/// Update the media metadata shown in the system notification / lock screen.
pub fn set_metadata(title: &str, artist: &str, duration_ms: u64) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        android::set_metadata(title, artist, duration_ms)
    }
    #[cfg(target_os = "ios")]
    {
        ios::set_metadata(title, artist, duration_ms)
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (title, artist, duration_ms);
        Err("media_session is only available on iOS and Android".into())
    }
}

/// Update the playback state shown in system controls.
pub fn set_playback_state(is_playing: bool, position_ms: u64, speed: f32) -> Result<(), String> {
    if !speed.is_finite() || speed < 0.0 {
        return Err("Playback speed must be finite and non-negative".into());
    }
    #[cfg(target_os = "android")]
    {
        android::set_playback_state(is_playing, position_ms, speed)
    }
    #[cfg(target_os = "ios")]
    {
        ios::set_playback_state(is_playing, position_ms, speed)
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (is_playing, position_ms, speed);
        Err("media_session is only available on iOS and Android".into())
    }
}

/// Release the media session and dismiss the notification.
pub fn release() -> Result<(), String> {
    *ACTION_CALLBACK.lock().unwrap() = None;
    *SEEK_CALLBACK.lock().unwrap() = None;
    #[cfg(target_os = "android")]
    {
        android::release()
    }
    #[cfg(target_os = "ios")]
    {
        ios::release()
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        Err("media_session is only available on iOS and Android".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn callbacks_can_replace_themselves_and_release_clears_both() {
        let seen = Arc::new(AtomicU64::new(0));
        let result = seen.clone();
        set_action_handler(move |_| {
            let result = result.clone();
            set_seek_handler(move |position| {
                result.store(position, Ordering::SeqCst);
            });
            set_action_handler(|_| {});
        });
        assert!(notify_action(MediaAction::Play));
        assert!(notify_seek(1234));
        assert_eq!(seen.load(Ordering::SeqCst), 1234);
        *ACTION_CALLBACK.lock().unwrap() = None;
        *SEEK_CALLBACK.lock().unwrap() = None;
        assert!(!notify_seek(99));
        assert!(!notify_action(MediaAction::Play));
        assert_eq!(seen.load(Ordering::SeqCst), 1234);
    }

    #[test]
    fn invalid_speed_is_rejected_before_native_dispatch() {
        for speed in [f32::NAN, f32::INFINITY, -1.0] {
            assert!(set_playback_state(true, 0, speed)
                .unwrap_err()
                .contains("speed"));
        }
    }
}
