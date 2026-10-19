use super::{events, LocationAccuracy, LocationSettings, Position};
use crate::android::jni as helpers;
use jni::objects::{JObject, JValue};
const HELPER_CLASS: &str = "dev.gpui.mobile.GpuiLocation";

pub fn is_location_service_enabled() -> Result<bool, String> {
    helpers::with_env(|env| {
        let activity = helpers::activity(env)?;
        let cls = helpers::find_app_class(env, HELPER_CLASS)?;
        env.call_static_method(
            &cls,
            jni::jni_str!("isLocationEnabled"),
            jni::jni_sig!("(Landroid/app/Activity;)Z"),
            &[JValue::Object(&activity)],
        )
        .and_then(|v| v.z())
        .map_err(|e| {
            env.exception_clear();
            e.to_string()
        })
    })
}
pub fn get_current_position(settings: &LocationSettings) -> Result<Position, String> {
    // Both the Java UI thread and GPUI's native UI thread own a looper.
    if !unsafe { ndk_sys::ALooper_forThread() }.is_null() {
        return Err("Use get_current_position_async on a looper/UI thread".into());
    }
    futures::executor::block_on(super::get_current_position_async(settings))
}
pub fn get_last_known_position() -> Result<Option<Position>, String> {
    helpers::with_env(|env| {
        let activity = helpers::activity(env)?;
        let cls = helpers::find_app_class(env, HELPER_CLASS)?;
        let value = env
            .call_static_method(
                &cls,
                jni::jni_str!("getLastKnownPosition"),
                jni::jni_sig!("(Landroid/app/Activity;)Ljava/lang/String;"),
                &[JValue::Object(&activity)],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;
        if value.is_null() {
            return Ok(None);
        }
        super::stream::parse_position(&helpers::get_string(env, &value)).map(Some)
    })
}
pub(super) fn start(
    id: u64,
    settings: &LocationSettings,
    once: bool,
    timeout_ms: u64,
    background: bool,
    previous: u64,
) -> Result<(), String> {
    let accuracy = match settings.accuracy {
        LocationAccuracy::Lowest => 0,
        LocationAccuracy::Low => 1,
        LocationAccuracy::Medium => 2,
        LocationAccuracy::High => 3,
        LocationAccuracy::Best => 4,
        LocationAccuracy::BestForNavigation => 5,
    };
    helpers::with_env(|env| {
        let activity = helpers::activity(env)?;
        let cls = helpers::find_app_class(env, HELPER_CLASS)?;
        env.call_static_method(
            &cls,
            jni::jni_str!("start"),
            jni::jni_sig!("(Landroid/app/Activity;JIDZJZJ)V"),
            &[
                JValue::Object(&activity),
                JValue::Long(id as i64),
                JValue::Int(accuracy),
                JValue::Double(settings.distance_filter),
                JValue::Bool(once),
                JValue::Long(timeout_ms as i64),
                JValue::Bool(background),
                JValue::Long(previous as i64),
            ],
        )
        .map(|_| ())
        .map_err(|e| {
            env.exception_clear();
            e.to_string()
        })
    })
}
pub(super) fn stop(id: u64) {
    if let Err(error) = helpers::with_env(|env| {
        let cls = helpers::find_app_class(env, HELPER_CLASS)?;
        env.call_static_method(
            &cls,
            jni::jni_str!("stop"),
            jni::jni_sig!("(J)V"),
            &[JValue::Long(id as i64)],
        )
        .map(|_| ())
        .map_err(|e| {
            env.exception_clear();
            e.to_string()
        })
    }) {
        log::error!("Failed to stop location subscription: {error}");
    }
}
/// JNI callback owns the local string for the duration of this call only.
#[no_mangle]
unsafe extern "system" fn Java_dev_gpui_mobile_GpuiLocation_nativeEvent(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
    id: i64,
    raw: jni::sys::jobject,
) {
    let _ = helpers::with_env(|env| {
        let object = JObject::from_raw(env, raw);
        if !events::send(id as u64, helpers::get_string(env, &object)) {
            stop(id as u64);
        }
        Ok(())
    });
}
