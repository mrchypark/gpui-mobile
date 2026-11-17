//! Audio playback for Android and iOS.
//!
//! Provides a cross-platform audio player API backed by:
//! - Android: MediaPlayer via JNI
//! - iOS: AVQueuePlayer/AVPlayerLooper via Objective-C
//!
//! Feature-gated behind `audio`.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

/// Events emitted by [`AudioPlayer::on_event`] and [`AudioPlayer::prepare_async`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerEvent {
    Ready { duration_ms: u64 },
    Position(u64),
    Completed,
    Error(String),
}

type EventCallback = Arc<dyn Fn(PlayerEvent) + Send + Sync + 'static>;

struct PlayerObserver {
    callback: EventCallback,
    stop: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
}

fn observers() -> &'static Mutex<HashMap<u32, PlayerObserver>> {
    static MAP: OnceLock<Mutex<HashMap<u32, PlayerObserver>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn emit_event(id: u32, event: PlayerEvent) {
    let cb = observers()
        .lock()
        .ok()
        .and_then(|map| map.get(&id).map(|obs| obs.callback.clone()));
    if let Some(cb) = cb {
        cb(event);
    }
}

fn stop_observer(id: u32) {
    if let Ok(mut map) = observers().lock() {
        if let Some(obs) = map.remove(&id) {
            obs.stop.store(true, Ordering::Release);
        }
    }
}

/// Audio player state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerState {
    Idle,
    Loading,
    Ready,
    Playing,
    Paused,
    Completed,
}

/// Loop mode.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LoopMode {
    #[default]
    Off,
    One,
    /// Repeats the current source, like One; this player has no playlist API.
    All,
}

/// Audio player instance (wraps a platform-specific player ID).
#[derive(Debug)]
#[allow(dead_code)]
pub struct AudioPlayer {
    id: u32,
}

impl AudioPlayer {
    /// Create a new audio player.
    pub fn new() -> Result<Self, String> {
        #[cfg(target_os = "android")]
        {
            android::create().map(|id| Self { id })
        }
        #[cfg(target_os = "ios")]
        {
            ios::create().map(|id| Self { id })
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Err("Audio not supported on this platform".into())
        }
    }

    /// Set the audio source from a URL (http/https/file).
    /// Returns the duration in milliseconds, or None if unknown.
    /// Android preparation waits for native readiness; call from a worker thread.
    pub fn set_url(&self, url: &str) -> Result<Option<u64>, String> {
        #[cfg(target_os = "android")]
        {
            android::set_url(self.id, url)
        }
        #[cfg(target_os = "ios")]
        {
            ios::set_url(self.id, url)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = url;
            Err("Audio not supported on this platform".into())
        }
    }

    /// Set the audio source from a file path.
    /// Android preparation waits for native readiness; call from a worker thread.
    pub fn set_file_path(&self, path: &str) -> Result<Option<u64>, String> {
        #[cfg(target_os = "android")]
        {
            android::set_url(self.id, path)
        }
        #[cfg(target_os = "ios")]
        {
            ios::set_file_path(self.id, path)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = path;
            Err("Audio not supported on this platform".into())
        }
    }

    /// Register a callback for [`PlayerEvent`] updates (ready, position, completed, error).
    pub fn on_event(
        &self,
        callback: impl Fn(PlayerEvent) + Send + Sync + 'static,
    ) -> Result<(), String> {
        let id = self.id;
        stop_observer(id);
        let stop = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicU64::new(1));
        let cb: EventCallback = Arc::new(callback);
        if let Ok(mut map) = observers().lock() {
            map.insert(
                id,
                PlayerObserver {
                    callback: cb,
                    stop: stop.clone(),
                    generation,
                },
            );
        }
        std::thread::spawn(move || {
            let mut last_pos: Option<u64> = None;
            let mut last_state: Option<PlayerState> = None;
            while !stop.load(Ordering::Acquire) {
                if let Ok(state) = Self::state_for_id(id) {
                    if state == PlayerState::Playing {
                        if let Ok(pos) = Self::position_for_id(id) {
                            if last_pos != Some(pos) {
                                last_pos = Some(pos);
                                emit_event(id, PlayerEvent::Position(pos));
                            }
                        }
                    }
                    if state == PlayerState::Completed && last_state != Some(PlayerState::Completed)
                    {
                        emit_event(id, PlayerEvent::Completed);
                    }
                    last_state = Some(state);
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
        Ok(())
    }

    /// Non-blocking preparation of a file path or URL; emits [`PlayerEvent::Ready`]
    /// or [`PlayerEvent::Error`] to the registered [`Self::on_event`] listener.
    pub fn prepare_async(&self, source: &str) {
        let id = self.id;
        let source = source.to_string();
        let gen = observers()
            .lock()
            .ok()
            .and_then(|map| {
                map.get(&id)
                    .map(|obs| obs.generation.fetch_add(1, Ordering::AcqRel) + 1)
            })
            .unwrap_or(0);
        std::thread::spawn(move || {
            let is_url = source.contains("://");
            let res = if is_url {
                Self::set_url_for_id(id, &source)
            } else {
                Self::set_file_path_for_id(id, &source)
            };
            let current_gen = observers().lock().ok().and_then(|map| {
                map.get(&id)
                    .map(|obs| obs.generation.load(Ordering::Acquire))
            });
            if gen != 0 && current_gen != Some(gen) {
                return;
            }
            match res {
                Ok(duration_ms) => emit_event(
                    id,
                    PlayerEvent::Ready {
                        duration_ms: duration_ms.unwrap_or(0),
                    },
                ),
                Err(err) => emit_event(id, PlayerEvent::Error(err)),
            }
        });
    }

    fn set_url_for_id(id: u32, url: &str) -> Result<Option<u64>, String> {
        #[cfg(target_os = "android")]
        {
            android::set_url(id, url)
        }
        #[cfg(target_os = "ios")]
        {
            ios::set_url(id, url)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = (id, url);
            Err("Audio not supported on this platform".into())
        }
    }

    fn set_file_path_for_id(id: u32, path: &str) -> Result<Option<u64>, String> {
        #[cfg(target_os = "android")]
        {
            android::set_url(id, path)
        }
        #[cfg(target_os = "ios")]
        {
            ios::set_file_path(id, path)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = (id, path);
            Err("Audio not supported on this platform".into())
        }
    }

    fn position_for_id(id: u32) -> Result<u64, String> {
        #[cfg(target_os = "android")]
        {
            android::get_position(id)
        }
        #[cfg(target_os = "ios")]
        {
            ios::get_position(id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = id;
            Err("Audio not supported on this platform".into())
        }
    }

    fn state_for_id(id: u32) -> Result<PlayerState, String> {
        #[cfg(target_os = "android")]
        {
            android::get_state(id)
        }
        #[cfg(target_os = "ios")]
        {
            ios::get_state(id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = id;
            Err("Audio not supported on this platform".into())
        }
    }

    /// Start or resume playback.
    pub fn play(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            android::play(self.id)
        }
        #[cfg(target_os = "ios")]
        {
            ios::play(self.id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Err("Audio not supported on this platform".into())
        }
    }

    /// Pause playback.
    pub fn pause(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            android::pause(self.id)
        }
        #[cfg(target_os = "ios")]
        {
            ios::pause(self.id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Err("Audio not supported on this platform".into())
        }
    }

    /// Stop playback and release resources.
    pub fn stop(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            android::stop(self.id)
        }
        #[cfg(target_os = "ios")]
        {
            ios::stop(self.id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Err("Audio not supported on this platform".into())
        }
    }

    /// Seek to position in milliseconds.
    pub fn seek(&self, position_ms: u64) -> Result<(), String> {
        if position_ms > i64::MAX as u64 {
            return Err("Seek position exceeds native time range".into());
        }
        #[cfg(target_os = "android")]
        {
            android::seek(self.id, position_ms)
        }
        #[cfg(target_os = "ios")]
        {
            ios::seek(self.id, position_ms)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = position_ms;
            Err("Audio not supported on this platform".into())
        }
    }

    /// Set volume (0.0 to 1.0).
    pub fn set_volume(&self, volume: f32) -> Result<(), String> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err("Volume must be finite and between 0 and 1".into());
        }
        #[cfg(target_os = "android")]
        {
            android::set_volume(self.id, volume)
        }
        #[cfg(target_os = "ios")]
        {
            ios::set_volume(self.id, volume)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = volume;
            Err("Audio not supported on this platform".into())
        }
    }

    /// Set playback speed (1.0 = normal).
    pub fn set_speed(&self, speed: f32) -> Result<(), String> {
        if !speed.is_finite() || speed <= 0.0 {
            return Err("Playback speed must be finite and positive".into());
        }
        #[cfg(target_os = "android")]
        {
            android::set_speed(self.id, speed)
        }
        #[cfg(target_os = "ios")]
        {
            ios::set_speed(self.id, speed)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = speed;
            Err("Audio not supported on this platform".into())
        }
    }

    /// Set loop mode.
    pub fn set_loop_mode(&self, mode: LoopMode) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            android::set_loop_mode(self.id, mode)
        }
        #[cfg(target_os = "ios")]
        {
            ios::set_loop_mode(self.id, mode)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = mode;
            Err("Audio not supported on this platform".into())
        }
    }

    /// Get current playback position in milliseconds.
    pub fn position(&self) -> Result<u64, String> {
        #[cfg(target_os = "android")]
        {
            android::get_position(self.id)
        }
        #[cfg(target_os = "ios")]
        {
            ios::get_position(self.id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Err("Audio not supported on this platform".into())
        }
    }

    /// Get total duration in milliseconds (0 if unknown).
    pub fn duration(&self) -> Result<u64, String> {
        #[cfg(target_os = "android")]
        {
            android::get_duration(self.id)
        }
        #[cfg(target_os = "ios")]
        {
            ios::get_duration(self.id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Err("Audio not supported on this platform".into())
        }
    }

    /// Get current player state.
    pub fn state(&self) -> Result<PlayerState, String> {
        #[cfg(target_os = "android")]
        {
            android::get_state(self.id)
        }
        #[cfg(target_os = "ios")]
        {
            ios::get_state(self.id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Err("Audio not supported on this platform".into())
        }
    }

    /// Check if currently playing.
    pub fn is_playing(&self) -> Result<bool, String> {
        #[cfg(target_os = "android")]
        {
            android::is_playing(self.id)
        }
        #[cfg(target_os = "ios")]
        {
            ios::is_playing(self.id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Err("Audio not supported on this platform".into())
        }
    }

    fn dispose(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            android::dispose(self.id)
        }
        #[cfg(target_os = "ios")]
        {
            ios::dispose(self.id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Ok(())
        }
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        stop_observer(self.id);
        let _ = self.dispose();
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;

    #[test]
    fn event_registry_dispatches_and_stops_cleanly() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = seen.clone();
        let stop = Arc::new(AtomicBool::new(false));
        observers().lock().unwrap().insert(
            9999,
            PlayerObserver {
                callback: Arc::new(move |ev| seen_clone.lock().unwrap().push(ev)),
                stop,
                generation: Arc::new(AtomicU64::new(0)),
            },
        );
        emit_event(9999, PlayerEvent::Ready { duration_ms: 1200 });
        emit_event(9999, PlayerEvent::Position(450));
        emit_event(9999, PlayerEvent::Completed);
        stop_observer(9999);
        emit_event(9999, PlayerEvent::Error("ignored".into()));
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                PlayerEvent::Ready { duration_ms: 1200 },
                PlayerEvent::Position(450),
                PlayerEvent::Completed,
            ]
        );
    }
}
