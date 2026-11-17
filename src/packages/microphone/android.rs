use super::{AudioFormat, AudioLibraryEntry, Recording, RecordingConfig};
use crate::android::jni::{self as jni_helpers, get_string, JniExt};
use jni::objects::{JClass, JFloatArray, JObject, JValue};
use std::sync::{Arc, Mutex};

const HELPER_CLASS: &str = "dev.gpui.mobile.GpuiMicrophone";

type PcmCallback = Arc<Mutex<Box<dyn Fn(&[f32]) + Send + 'static>>>;
static PCM_CALLBACK: Mutex<Option<PcmCallback>> = Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_gpui_mobile_GpuiMicrophone_nativeOnPcmFrames(
    mut env: jni::EnvUnowned<'_>,
    _class: JClass<'_>,
    frames: JFloatArray<'_>,
) {
    let cb = PCM_CALLBACK
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().cloned());
    let Some(cb) = cb else {
        return;
    };
    let _ = env.with_env(|env| -> jni::errors::Result<()> {
        let len = frames.len(env)?;
        if len == 0 {
            return Ok(());
        }
        let mut buf = vec![0.0f32; len];
        frames.get_region(env, 0, &mut buf)?;
        if let Ok(guard) = cb.lock() {
            guard(&buf);
        }
        Ok(())
    });
}

pub fn is_available() -> bool {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("isAvailable"),
                jni::jni_sig!("(Landroid/app/Activity;)Z"),
                &[JValue::Object(&activity)],
            )
            .and_then(|v| v.z())
            .e()?;

        Ok(result)
    })
    .unwrap_or(false)
}

pub fn start_recording(config: &RecordingConfig) -> Result<String, String> {
    let format = match config.format {
        AudioFormat::Aac => 0,
        AudioFormat::Wav => 1,
        AudioFormat::Amr => 2,
    };
    let sample_rate = config.sample_rate as i32;
    let channels = config.channels as i32;
    let bit_rate = config.bit_rate as i32;

    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("startRecording"),
                jni::jni_sig!("(Landroid/app/Activity;IIII)Ljava/lang/String;"),
                &[
                    JValue::Object(&activity),
                    JValue::Int(format),
                    JValue::Int(sample_rate),
                    JValue::Int(channels),
                    JValue::Int(bit_rate),
                ],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;

        if result.is_null() {
            return Err("Failed to start recording".into());
        }

        let path = get_string(env, &result);
        if path.is_empty() {
            Err("Failed to start recording: empty path".into())
        } else {
            Ok(path)
        }
    })
}

pub fn start_recording_stream(
    config: &RecordingConfig,
    on_pcm: Box<dyn Fn(&[f32]) + Send + 'static>,
) -> Result<String, String> {
    let sample_rate = config.sample_rate as i32;
    let channels = config.channels as i32;
    let wrapped: PcmCallback = Arc::new(Mutex::new(on_pcm));
    {
        let mut guard = PCM_CALLBACK
            .lock()
            .map_err(|_| "PCM callback lock poisoned".to_string())?;
        *guard = Some(wrapped);
    }

    let started = jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("startRecordingStream"),
                jni::jni_sig!("(Landroid/app/Activity;II)Ljava/lang/String;"),
                &[
                    JValue::Object(&activity),
                    JValue::Int(sample_rate),
                    JValue::Int(channels),
                ],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;

        if result.is_null() {
            return Err("Failed to start streaming recording".into());
        }

        let path = get_string(env, &result);
        if path.is_empty() {
            Err("Failed to start streaming recording: empty path".into())
        } else {
            Ok(path)
        }
    });
    if started.is_err() {
        if let Ok(mut guard) = PCM_CALLBACK.lock() {
            *guard = None;
        }
    }
    started
}

pub fn query_audio_entries(since_ms: u64) -> Result<Vec<AudioLibraryEntry>, String> {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("queryAudioEntries"),
                jni::jni_sig!("(Landroid/app/Activity;J)[Ljava/lang/String;"),
                &[JValue::Object(&activity), JValue::Long(since_ms as i64)],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;
        if result.is_null() {
            return Ok(Vec::new());
        }
        let arr = unsafe { jni::objects::JObjectArray::<JObject>::from_raw(env, result.as_raw()) };
        let len = arr.len(env).e()?;
        let mut entries = Vec::with_capacity(len / 6);
        for base in (0..len).step_by(6) {
            if base + 5 >= len {
                break;
            }
            let mut get = |idx| -> Result<String, String> {
                let obj: JObject = arr.get_element(env, idx).e()?;
                Ok(get_string(env, &obj))
            };
            let uri = get(base)?;
            let display_name = get(base + 1)?;
            let dur = get(base + 2)?;
            let added = get(base + 3)?;
            let size = get(base + 4)?;
            let mime = get(base + 5)?;
            entries.push(AudioLibraryEntry {
                uri,
                display_name,
                duration_ms: (!dur.is_empty()).then(|| dur.parse::<u64>().ok()).flatten(),
                added_ms: (!added.is_empty())
                    .then(|| added.parse::<u64>().ok())
                    .flatten(),
                size_bytes: (!size.is_empty())
                    .then(|| size.parse::<u64>().ok())
                    .flatten(),
                mime_type: (!mime.is_empty()).then_some(mime),
            });
        }
        Ok(entries)
    })
}

pub fn stop_recording() -> Result<Recording, String> {
    if let Ok(mut guard) = PCM_CALLBACK.lock() {
        *guard = None;
    }
    jni_helpers::with_env(|env| {
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("stopRecording"),
                jni::jni_sig!("()Ljava/lang/String;"),
                &[],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;

        if result.is_null() {
            return Err("Not recording or failed to stop".into());
        }

        let result_str = get_string(env, &result);
        // Format: "path|duration_ms"
        let parts: Vec<&str> = result_str.splitn(2, '|').collect();
        if parts.len() < 2 {
            return Err("Invalid recording result".into());
        }

        Ok(Recording {
            path: parts[0].to_string(),
            duration_ms: parts[1].parse().unwrap_or(0),
        })
    })
}

pub fn is_recording() -> bool {
    jni_helpers::with_env(|env| {
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("isRecording"),
                jni::jni_sig!("()Z"),
                &[],
            )
            .and_then(|v| v.z())
            .e()?;
        Ok(result)
    })
    .unwrap_or(false)
}

pub fn pause_recording() -> Result<(), String> {
    jni_helpers::with_env(|env| {
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let success = env
            .call_static_method(
                &cls,
                jni::jni_str!("pauseRecording"),
                jni::jni_sig!("()Z"),
                &[],
            )
            .and_then(|v| v.z())
            .e()?;
        if success {
            Ok(())
        } else {
            Err("Failed to pause recording".into())
        }
    })
}

pub fn resume_recording() -> Result<(), String> {
    jni_helpers::with_env(|env| {
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let success = env
            .call_static_method(
                &cls,
                jni::jni_str!("resumeRecording"),
                jni::jni_sig!("()Z"),
                &[],
            )
            .and_then(|v| v.z())
            .e()?;
        if success {
            Ok(())
        } else {
            Err("Failed to resume recording".into())
        }
    })
}

pub fn get_amplitude() -> Result<f64, String> {
    jni_helpers::with_env(|env| {
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("getAmplitude"),
                jni::jni_sig!("()D"),
                &[],
            )
            .and_then(|v| v.d())
            .e()?;
        Ok(result)
    })
}
