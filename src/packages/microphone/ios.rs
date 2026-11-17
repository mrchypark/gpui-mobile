use super::{AudioFormat, AudioLibraryEntry, Recording, RecordingConfig};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

struct RecorderState {
    recorder: *mut AnyObject, // AVAudioRecorder
    path: String,
    stream_stop: Option<Arc<AtomicBool>>,
}

unsafe impl Send for RecorderState {}

static STATE: Mutex<Option<RecorderState>> = Mutex::new(None);

pub fn is_available() -> bool {
    unsafe {
        let session: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];
        msg_send![session, isInputAvailable]
    }
}

pub fn start_recording(config: &RecordingConfig) -> Result<String, String> {
    if config.format == AudioFormat::Amr {
        return Err("AMR recording is Android-only".into());
    }
    let mut guard = STATE.lock().unwrap();
    if guard.is_some() {
        return Err("Already recording".into());
    }

    unsafe {
        // Generate output path
        let ext = match config.format {
            AudioFormat::Aac => "m4a",
            AudioFormat::Wav => "wav",
            AudioFormat::Amr => "amr",
        };
        let uuid_str = uuid::Uuid::new_v4().to_string();
        let file_name = format!("recording_{}.{}", uuid_str, ext);
        let file_path = std::env::temp_dir().join(&file_name);
        let path_str = file_path.to_string_lossy().into_owned();

        // Create NSURL
        let ns_path = nsstring(&path_str);
        let file_url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: ns_path];
        if file_url.is_null() {
            return Err("Failed to create file URL".into());
        }

        // Build settings dictionary
        let keys: [*mut AnyObject; 4] = [
            nsstring("AVFormatIDKey"),
            nsstring("AVSampleRateKey"),
            nsstring("AVNumberOfChannelsKey"),
            nsstring("AVEncoderBitRateKey"),
        ];

        let format_id: i32 = match config.format {
            AudioFormat::Aac => 1633772320, // kAudioFormatMPEG4AAC
            AudioFormat::Wav => 1819304813, // kAudioFormatLinearPCM
            AudioFormat::Amr => 1935764850, // kAudioFormatAMR (not well supported on iOS)
        };

        let values: [*mut AnyObject; 4] = [
            number_with_int(format_id),
            number_with_float(config.sample_rate as f64),
            number_with_int(config.channels as i32),
            number_with_int(config.bit_rate as i32),
        ];

        let settings: *mut AnyObject = msg_send![class!(NSMutableDictionary),
            dictionaryWithObjects: values.as_ptr(),
            forKeys: keys.as_ptr(),
            count: 4usize
        ];
        if config.format == AudioFormat::Wav {
            let _: () = msg_send![settings, removeObjectForKey: nsstring("AVEncoderBitRateKey")];
            for (key, value) in [
                ("AVLinearPCMBitDepthKey", 16),
                ("AVLinearPCMIsFloatKey", 0),
                ("AVLinearPCMIsBigEndianKey", 0),
            ] {
                let _: () =
                    msg_send![settings, setObject: number_with_int(value), forKey: nsstring(key)];
            }
        }

        // Create AVAudioRecorder
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let recorder: *mut AnyObject = msg_send![class!(AVAudioRecorder), alloc];
        let recorder: *mut AnyObject = msg_send![recorder,
            initWithURL: file_url,
            settings: settings,
            error: &mut error as *mut *mut AnyObject
        ];

        if recorder.is_null() || !error.is_null() {
            let err_msg = if !error.is_null() {
                let desc: *mut AnyObject = msg_send![error, localizedDescription];
                objc_string_to_rust(desc)
            } else {
                "Failed to create recorder".to_string()
            };
            if !recorder.is_null() {
                let _: () = msg_send![recorder, release];
            }
            return Err(err_msg);
        }

        // Enable metering for amplitude
        let _: () = msg_send![recorder, setMeteringEnabled: true];

        // Activate audio session
        let session: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];
        let record_category = nsstring("AVAudioSessionCategoryRecord");
        let mut session_error: *mut AnyObject = std::ptr::null_mut();
        let category_ok: bool = msg_send![session,
            setCategory: record_category,
            error: &mut session_error as *mut *mut AnyObject
        ];
        if !category_ok {
            let _: () = msg_send![recorder, release];
            return Err("Failed to configure recording audio session".into());
        }
        let active: bool = msg_send![session,
            setActive: true,
            error: &mut session_error as *mut *mut AnyObject
        ];
        if !active {
            let _: () = msg_send![recorder, release];
            return Err("Failed to activate recording audio session".into());
        }

        // Start recording
        let ok: bool = msg_send![recorder, record];
        if !ok {
            let _: () = msg_send![recorder, release];
            let _: bool = msg_send![session, setActive: false, error: &mut session_error as *mut *mut AnyObject];
            return Err("Failed to start recording".into());
        }

        *guard = Some(RecorderState {
            recorder,
            path: path_str.clone(),
            stream_stop: None,
        });

        Ok(path_str)
    }
}

pub fn start_recording_stream(
    config: &RecordingConfig,
    on_pcm: Box<dyn Fn(&[f32]) + Send + 'static>,
) -> Result<String, String> {
    let path = start_recording(config)?;
    let stop = Arc::new(AtomicBool::new(false));
    if let Ok(mut guard) = STATE.lock() {
        if let Some(state) = guard.as_mut() {
            state.stream_stop = Some(stop.clone());
        }
    }
    let tail_path = path.clone();
    std::thread::spawn(move || {
        use std::io::{Read, Seek, SeekFrom};
        let mut offset = 44u64;
        let mut buf = [0u8; 4096];
        while !stop.load(Ordering::Acquire) {
            if let Ok(mut file) = std::fs::File::open(&tail_path) {
                if let Ok(len) = file.metadata().map(|m| m.len()) {
                    if len > offset + 1 && file.seek(SeekFrom::Start(offset)).is_ok() {
                        let mut available = ((len - offset) & !1) as usize;
                        while available > 0 {
                            let want = available.min(buf.len()) & !1;
                            if want == 0 {
                                break;
                            }
                            match file.read(&mut buf[..want]) {
                                Ok(n) if n >= 2 => {
                                    let even = n & !1;
                                    offset += even as u64;
                                    available = available.saturating_sub(even);
                                    let samples = super::pcm16_le_to_f32(&buf[..even]);
                                    if !samples.is_empty() {
                                        on_pcm(&samples);
                                    }
                                }
                                _ => break,
                            }
                        }
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
    Ok(path)
}

pub fn query_audio_entries(since_ms: u64) -> Result<Vec<AudioLibraryEntry>, String> {
    let mut dirs = vec![std::env::temp_dir()];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(std::path::PathBuf::from(home).join("Documents"));
    }
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(ext) = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
            else {
                continue;
            };
            let mime = match ext.as_str() {
                "m4a" | "mp4" => "audio/mp4",
                "wav" => "audio/wav",
                "mp3" => "audio/mpeg",
                "aac" => "audio/aac",
                "caf" => "audio/x-caf",
                _ => continue,
            };
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let added_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
            if since_ms > 0 && added_ms.unwrap_or(0) < since_ms {
                continue;
            }
            let display_name = entry.file_name().to_string_lossy().into_owned();
            out.push(AudioLibraryEntry {
                uri: path.to_string_lossy().into_owned(),
                display_name,
                duration_ms: None,
                added_ms,
                size_bytes: Some(meta.len()),
                mime_type: Some(mime.to_string()),
            });
        }
    }
    Ok(out)
}

pub fn stop_recording() -> Result<Recording, String> {
    let mut guard = STATE.lock().unwrap();
    let state = guard.take().ok_or("Not recording")?;
    if let Some(stop) = state.stream_stop {
        stop.store(true, Ordering::Release);
    }

    unsafe {
        let duration: f64 = msg_send![state.recorder, currentTime];
        let _: () = msg_send![state.recorder, stop];
        let _: () = msg_send![state.recorder, release];
        let duration_ms = (duration * 1000.0) as u64;

        // Deactivate audio session
        let session: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let _: bool = msg_send![session,
            setActive: false,
            error: &mut error as *mut *mut AnyObject
        ];

        Ok(Recording {
            path: state.path,
            duration_ms,
        })
    }
}

pub fn is_recording() -> bool {
    let guard = STATE.lock().unwrap();
    if let Some(state) = guard.as_ref() {
        unsafe { msg_send![state.recorder, isRecording] }
    } else {
        false
    }
}

pub fn pause_recording() -> Result<(), String> {
    let guard = STATE.lock().unwrap();
    let state = guard.as_ref().ok_or("Not recording")?;
    unsafe {
        let _: () = msg_send![state.recorder, pause];
    }
    Ok(())
}

pub fn resume_recording() -> Result<(), String> {
    let guard = STATE.lock().unwrap();
    let state = guard.as_ref().ok_or("Not recording")?;
    unsafe {
        let ok: bool = msg_send![state.recorder, record];
        if ok {
            Ok(())
        } else {
            Err("Failed to resume recording".into())
        }
    }
}

pub fn get_amplitude() -> Result<f64, String> {
    let guard = STATE.lock().unwrap();
    let state = guard.as_ref().ok_or("Not recording")?;
    unsafe {
        let recording: bool = msg_send![state.recorder, isRecording];
        if !recording {
            return Ok(0.0);
        }
        let _: () = msg_send![state.recorder, updateMeters];
        let level: f32 = msg_send![state.recorder, averagePowerForChannel: 0i64];
        // Convert from dB (range: -160 to 0) to linear (0.0 to 1.0)
        let linear = if level <= -160.0 {
            0.0
        } else {
            10.0_f64.powf(level as f64 / 20.0)
        };
        Ok(linear)
    }
}

// Helpers

use crate::ios::util::nsstring;

unsafe fn number_with_int(val: i32) -> *mut AnyObject {
    msg_send![class!(NSNumber), numberWithInt: val]
}

unsafe fn number_with_float(val: f64) -> *mut AnyObject {
    msg_send![class!(NSNumber), numberWithDouble: val]
}

unsafe fn objc_string_to_rust(ns: *mut AnyObject) -> String {
    if ns.is_null() {
        return String::new();
    }
    let cstr: *const std::ffi::c_char = msg_send![ns, UTF8String];
    if cstr.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(cstr)
        .to_string_lossy()
        .into_owned()
}
