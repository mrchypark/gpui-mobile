use super::{BarometerData, SensorAvailability, SensorData};
use crate::android::jni as jni_helpers;
use jni::objects::{JDoubleArray, JValue};

const HELPER_CLASS: &str = "dev.gpui.mobile.GpuiSensors";

pub fn available_sensors() -> SensorAvailability {
    let bits = jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        env.call_static_method(
            &cls,
            jni::jni_str!("available"),
            jni::jni_sig!("(Landroid/app/Activity;)I"),
            &[JValue::Object(&activity)],
        )
        .and_then(|v| v.i())
        .map_err(|e| {
            env.exception_clear();
            e.to_string()
        })
    })
    .unwrap_or_default();
    SensorAvailability {
        accelerometer: bits & 1 != 0,
        gyroscope: bits & 2 != 0,
        magnetometer: bits & 4 != 0,
        barometer: bits & 8 != 0,
    }
}

fn read(sensor_type: i32) -> Option<[f64; 3]> {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("read"),
                jni::jni_sig!("(Landroid/app/Activity;I)[D"),
                &[JValue::Object(&activity), JValue::Int(sensor_type)],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;
        if result.is_null() {
            return Ok(None);
        }
        let array = unsafe { JDoubleArray::from_raw(env, result.into_raw() as _) };
        let mut values = [0.0; 3];
        array.get_region(env, 0, &mut values).map_err(|e| {
            env.exception_clear();
            e.to_string()
        })?;
        Ok(Some(values))
    })
    .unwrap_or_else(|error| {
        log::warn!("Sensor read failed: {error}");
        None
    })
}
fn axes(sensor_type: i32) -> Option<SensorData> {
    read(sensor_type).map(|[x, y, z]| SensorData { x, y, z })
}
pub fn accelerometer() -> Option<SensorData> {
    axes(1)
}
pub fn gyroscope() -> Option<SensorData> {
    axes(4)
}
pub fn magnetometer() -> Option<SensorData> {
    axes(2)
}
pub fn barometer() -> Option<BarometerData> {
    read(6).map(|values| BarometerData {
        pressure: values[0],
    })
}
