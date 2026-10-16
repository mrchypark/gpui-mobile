use super::VideoInfo;
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use std::collections::HashMap;
use std::sync::Mutex;

#[path = "../audio/ios_loop.rs"]
mod ios_loop;

/// Global storage for AVPlayer instances keyed by player ID.
static PLAYERS: Mutex<Option<HashMap<u32, PlayerEntry>>> = Mutex::new(None);

/// Next available player ID.
static NEXT_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

struct PlayerEntry {
    /// The AVPlayer instance (retained).
    player: *mut AnyObject,
    /// Whether looping is enabled.
    looping: bool,
    loops: ios_loop::Looping,
    speed: f32,
}

// AVPlayer pointers are safe to send across threads when properly retained.
unsafe impl Send for PlayerEntry {}

fn with_players<T>(f: impl FnOnce(&mut HashMap<u32, PlayerEntry>) -> T) -> T {
    let mut guard = PLAYERS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

fn with_player<T>(
    id: u32,
    f: impl FnOnce(&mut PlayerEntry) -> Result<T, String>,
) -> Result<T, String> {
    with_players(|map| match map.get_mut(&id) {
        Some(entry) => f(entry),
        None => Err(format!("VideoPlayer {id} not found")),
    })
}

/// Create a new AVPlayer and return its ID.
pub fn create_player() -> Result<u32, String> {
    unsafe {
        let player: *mut AnyObject = msg_send![class!(AVQueuePlayer), alloc];
        let player: *mut AnyObject = msg_send![player, init];
        if player.is_null() {
            return Err("Failed to create AVPlayer".into());
        }

        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        with_players(|map| {
            map.insert(
                id,
                PlayerEntry {
                    player,
                    looping: false,
                    loops: Default::default(),
                    speed: 1.0,
                },
            );
        });

        Ok(id)
    }
}

/// Get the raw AVPlayer pointer for a given player ID.
///
/// Legacy borrowed accessor; concurrent disposal invalidates the result.
/// Internal layer construction must use retain_player instead.
pub fn get_player_ptr(id: u32) -> Option<*mut AnyObject> {
    with_players(|map| map.get(&id).map(|entry| entry.player))
}

pub(super) fn retain_player(id: u32) -> Option<objc2::rc::Retained<AnyObject>> {
    with_players(|map| {
        let entry = map.get(&id)?;
        // The registry's +1 cannot be released by dispose until this retain finishes.
        unsafe { objc2::rc::Retained::retain(entry.player) }
    })
}

pub fn set_url(id: u32, url: &str) -> Result<VideoInfo, String> {
    load_source(id, url, false)
}

pub fn set_file_path(id: u32, path: &str) -> Result<VideoInfo, String> {
    load_source(id, path, true)
}

fn load_source(id: u32, source: &str, file: bool) -> Result<VideoInfo, String> {
    objc2::rc::autoreleasepool(|_| load_source_inner(id, source, file))
}

fn load_source_inner(id: u32, source: &str, file: bool) -> Result<VideoInfo, String> {
    unsafe {
        let main: bool = msg_send![class!(NSThread), isMainThread];
        if main { return Err("Load media from a worker thread".into()); }
    }
    let item = with_player(id, |entry| unsafe {
        let text = make_nsstring(source);
        let url: *mut AnyObject = if file {
            msg_send![class!(NSURL), fileURLWithPath: text]
        } else {
            msg_send![class!(NSURL), URLWithString: text]
        };
        if url.is_null() { return Err("Invalid media URL".into()); }
        let item: *mut AnyObject = msg_send![class!(AVPlayerItem), playerItemWithURL: url];
        if item.is_null() { return Err("Failed to create video item".into()); }
        entry.loops.clear();
        let _: () = msg_send![entry.player, removeAllItems];
        let _: () = msg_send![entry.player, replaceCurrentItemWithPlayerItem: item];
        let _: () = msg_send![entry.player, setActionAtItemEnd: 1isize];
        let _: *mut AnyObject = msg_send![item, retain];
        Ok(LoadingItem(item))
    })?;
    // No registry lock while native loading waits. Disposal/source replacement can
    // proceed, and the retained item remains alive until this operation returns.
    if let Err(error) = unsafe { wait_for_item_ready(item.0) } {
        // Timeout can leave status Unknown; discard this load before later play,
        // but never discard a concurrently installed replacement.
        let _ = with_player(id, |entry| unsafe {
            let current: *mut AnyObject = msg_send![entry.player, currentItem];
            if current == item.0 {
                let _: () = msg_send![entry.player, removeAllItems];
            }
            Ok(())
        });
        return Err(error);
    }
    with_player(id, |entry| unsafe {
        let current: *mut AnyObject = msg_send![entry.player, currentItem];
        if current != item.0 { return Err("Video source changed while loading".into()); }
        let info = get_video_info(entry.player)?;
        if entry.looping { entry.loops.configure(entry.player, true)?; }
        Ok(info)
    })
}

struct LoadingItem(*mut AnyObject);
impl Drop for LoadingItem {
    fn drop(&mut self) { unsafe { let _: () = msg_send![self.0, release]; } }
}

pub fn play(id: u32) -> Result<(), String> {
    with_player(id, |entry| {
        unsafe {
            ios_loop::require_playable(entry.player)?;
            let _: () = msg_send![entry.player, setRate: entry.speed];
        }
        Ok(())
    })
}

pub fn pause(id: u32) -> Result<(), String> {
    with_player(id, |entry| {
        unsafe {
            let _: () = msg_send![entry.player, pause];
        }
        Ok(())
    })
}

pub fn seek(id: u32, position_ms: u64) -> Result<(), String> {
    with_player(id, |entry| {
        unsafe {
            let time = make_cmtime(position_ms);
            let _: () = msg_send![entry.player, seekToTime: time];
        }
        Ok(())
    })
}

pub fn set_volume(id: u32, volume: f32) -> Result<(), String> {
    with_player(id, |entry| {
        unsafe {
            let _: () = msg_send![entry.player, setVolume: volume];
        }
        Ok(())
    })
}

pub fn set_speed(id: u32, speed: f32) -> Result<(), String> {
    with_player(id, |entry| {
        unsafe {
            let rate: f32 = msg_send![entry.player, rate];
            entry.speed = speed;
            if rate != 0.0 { let _: () = msg_send![entry.player, setRate: speed]; }
        }
        Ok(())
    })
}

pub fn set_looping(id: u32, looping: bool) -> Result<(), String> {
    with_player(id, |entry| unsafe {
        entry.loops.configure(entry.player, looping)?;
        entry.looping = looping;
        Ok(())
    })
}
pub fn position(id: u32) -> Result<u64, String> {
    with_player(id, |entry| unsafe {
        let time: CMTime = msg_send![entry.player, currentTime];
        Ok(cmtime_to_ms(time))
    })
}

pub fn duration(id: u32) -> Result<u64, String> {
    with_player(id, |entry| unsafe {
        let item: *mut AnyObject = msg_send![entry.player, currentItem];
        if item.is_null() {
            return Err("No current item".into());
        }
        let dur: CMTime = msg_send![item, duration];
        Ok(cmtime_to_ms(dur))
    })
}

pub fn video_size(id: u32) -> Result<(u32, u32), String> {
    with_player(id, |entry| unsafe {
        let item: *mut AnyObject = msg_send![entry.player, currentItem];
        if item.is_null() {
            return Err("No current item".into());
        }
        let size: CGSize = msg_send![item, presentationSize];
        Ok((size.width as u32, size.height as u32))
    })
}

pub fn is_playing(id: u32) -> Result<bool, String> {
    with_player(id, |entry| unsafe {
        let rate: f32 = msg_send![entry.player, rate];
        Ok(rate > 0.0)
    })
}

pub fn dispose(id: u32) -> Result<(), String> {
    let entry = with_players(|map| map.remove(&id));
    if let Some(mut entry) = entry {
        unsafe {
            let _: () = msg_send![entry.player, pause];
            entry.loops.clear();
            let _: () = msg_send![entry.player, release];
        }
    }
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// CMTime layout as used by AVFoundation.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[
            Encoding::LongLong,
            Encoding::Int,
            Encoding::UInt,
            Encoding::LongLong,
        ],
    );
}

unsafe impl RefEncode for CMTime {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

/// CGSize layout.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[Encoding::Double, Encoding::Double]);
}

unsafe impl RefEncode for CGSize {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

/// CMTime flag indicating a valid time.
const CMTIME_FLAGS_VALID: u32 = 1;
/// CMTime flag indicating positive infinity.
const CMTIME_FLAGS_POSITIVE_INFINITY: u32 = 4;

/// Create a CMTime from milliseconds.
fn make_cmtime(ms: u64) -> CMTime {
    CMTime {
        value: ms as i64,
        timescale: 1000,
        flags: CMTIME_FLAGS_VALID,
        epoch: 0,
    }
}

/// Convert a CMTime to milliseconds.
fn cmtime_to_ms(time: CMTime) -> u64 {
    if time.flags & CMTIME_FLAGS_VALID == 0 {
        return 0;
    }
    if time.flags & CMTIME_FLAGS_POSITIVE_INFINITY != 0 {
        return 0;
    }
    if time.timescale <= 0 {
        return 0;
    }
    ((time.value as f64 / time.timescale as f64) * 1000.0).max(0.0) as u64
}

unsafe fn make_nsstring(s: &str) -> *mut AnyObject {
    crate::ios::util::nsstring(s)
}

/// Wait (up to 5 seconds) for the current AVPlayerItem to reach ReadyToPlay status.
unsafe fn wait_for_item_ready(item: *mut AnyObject) -> Result<(), String> {
    if item.is_null() {
        return Err("No player item".into());
    }

    // AVPlayerItemStatus: 0 = Unknown, 1 = ReadyToPlay, 2 = Failed
    for _ in 0..100 {
        let status: i64 = msg_send![item, status];
        match status {
            1 => return Ok(()),
            2 => return Err("AVPlayerItem failed to load".into()),
            _ => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    Err("Timed out waiting for AVPlayerItem to become ready".into())
}

/// Extract video info from the current player item.
unsafe fn get_video_info(player: *mut AnyObject) -> Result<VideoInfo, String> {
    let item: *mut AnyObject = msg_send![player, currentItem];
    if item.is_null() {
        return Err("No player item".into());
    }

    let dur: CMTime = msg_send![item, duration];
    let size: CGSize = msg_send![item, presentationSize];

    Ok(VideoInfo {
        duration_ms: cmtime_to_ms(dur),
        width: size.width as u32,
        height: size.height as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_rejects_empty_and_failed_source() {
        objc2::rc::autoreleasepool(|_| {
            let id = create_player().unwrap();
            assert!(play(id).is_err());
            let failure = std::thread::spawn(move || {
                set_file_path(id, "/gpui-nonexistent-media-regression.mp4")
            }).join().unwrap();
            assert!(failure.is_err());
            assert!(play(id).is_err());
            // A later valid source must clear the failed-item condition rather
            // than leaving a Rust-side sticky failure flag.
            let path = std::env::temp_dir().join(format!("gpui-play-{}.wav", uuid::Uuid::new_v4()));
            let mut wav = Vec::new();
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&16036u32.to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16u32.to_le_bytes());
            wav.extend_from_slice(&1u16.to_le_bytes());
            wav.extend_from_slice(&1u16.to_le_bytes());
            wav.extend_from_slice(&8000u32.to_le_bytes());
            wav.extend_from_slice(&16000u32.to_le_bytes());
            wav.extend_from_slice(&2u16.to_le_bytes());
            wav.extend_from_slice(&16u16.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&16000u32.to_le_bytes());
            wav.resize(16044, 0);
            std::fs::write(&path, wav).unwrap();
            let source = path.to_string_lossy().into_owned();
            let recovered = std::thread::spawn(move || set_file_path(id, &source)).join().unwrap();
            let playback = recovered.and_then(|_| play(id));
            let _ = pause(id);
            dispose(id).unwrap();
            std::fs::remove_file(path).unwrap();
            assert!(playback.is_ok(), "valid source did not recover: {playback:?}");
        });
    }

    #[test]
    fn retained_player_survives_registry_disposal() {
        objc2::rc::autoreleasepool(|_| {
            let id = create_player().unwrap();
            let retained = retain_player(id).unwrap();
            std::thread::spawn(move || dispose(id).unwrap()).join().unwrap();
            assert!(retain_player(id).is_none());
            let alive: bool = unsafe { msg_send![&*retained, isKindOfClass: class!(AVQueuePlayer)] };
            assert!(alive);
            // The guard releases the last reference after the native call, with
            // no registry mutex held across thread joining or native layer work.
        });
    }
}
