use super::BatteryState;
use crate::android::jni as jni_helpers;
use jni::objects::JValue;

/// Android BatteryManager.EXTRA_* constants.
const BATTERY_STATUS_CHARGING: i32 = 2;
const BATTERY_STATUS_DISCHARGING: i32 = 3;
const BATTERY_STATUS_FULL: i32 = 5;
const BATTERY_STATUS_NOT_CHARGING: i32 = 4;

pub fn battery_level() -> i32 {
    let (level, scale, _) = match read_battery_sticky() {
        Some(v) => v,
        None => return -1,
    };
    percentage(level, scale)
}

fn percentage(level: i32, scale: i32) -> i32 {
    if level < 0 || scale <= 0 || level > scale {
        return -1;
    }
    ((i64::from(level) * 100) / i64::from(scale)) as i32
}

#[cfg(test)]
mod tests {
    #[test]
    fn battery_percentage_preserves_unknown_and_avoids_overflow() {
        assert_eq!(super::percentage(-1, 100), -1);
        assert_eq!(super::percentage(1, 0), -1);
        assert_eq!(super::percentage(i32::MAX, i32::MAX), 100);
        assert_eq!(super::percentage(1, 4), 25);
    }
}

pub fn battery_state() -> BatteryState {
    let (_, _, status) = match read_battery_sticky() {
        Some(v) => v,
        None => return BatteryState::Unknown,
    };
    match status {
        BATTERY_STATUS_CHARGING => BatteryState::Charging,
        BATTERY_STATUS_DISCHARGING | BATTERY_STATUS_NOT_CHARGING => BatteryState::Discharging,
        BATTERY_STATUS_FULL => BatteryState::Full,
        _ => BatteryState::Unknown,
    }
}

pub fn is_battery_save_mode() -> bool {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;

        // PowerManager pm = (PowerManager) context.getSystemService("power");
        let service_name = env.new_string("power").map_err(|e| e.to_string())?;
        let pm = match env
            .call_method(
                &activity,
                jni::jni_str!("getSystemService"),
                jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
                &[JValue::Object(&service_name)],
            )
            .and_then(|v| v.l())
        {
            Ok(o) if !o.is_null() => o,
            _ => {
                env.exception_clear();
                return Ok(false);
            }
        };

        // pm.isPowerSaveMode()
        match env
            .call_method(
                &pm,
                jni::jni_str!("isPowerSaveMode"),
                jni::jni_sig!("()Z"),
                &[],
            )
            .and_then(|v| v.z())
        {
            Ok(v) => Ok(v),
            Err(_) => {
                env.exception_clear();
                Ok(false)
            }
        }
    })
    .unwrap_or(false)
}

/// Read battery info from the sticky ACTION_BATTERY_CHANGED broadcast.
///
/// Returns `(level, scale, status)` or None on failure.
fn read_battery_sticky() -> Option<(i32, i32, i32)> {
    jni_helpers::with_env(|env| {
        let result = (|| {
        let activity = jni_helpers::activity(env)?;

        // IntentFilter filter = new IntentFilter(Intent.ACTION_BATTERY_CHANGED);
        let action = env.new_string("android.intent.action.BATTERY_CHANGED").map_err(|e| e.to_string())?;
        let filter = env
            .new_object(
                jni::jni_str!("android/content/IntentFilter"),
                jni::jni_sig!("(Ljava/lang/String;)V"),
                &[JValue::Object(&action)],
            )
            .map_err(|e| e.to_string())?;

        // Intent batteryStatus = context.registerReceiver(null, filter);
        let battery_intent = env
            .call_method(
                &activity,
                jni::jni_str!("registerReceiver"),
                jni::jni_sig!("(Landroid/content/BroadcastReceiver;Landroid/content/IntentFilter;)Landroid/content/Intent;"),
                &[JValue::Object(&jni::objects::JObject::null()), JValue::Object(&filter)],
            )
            .and_then(|v| v.l())
            .map_err(|e| e.to_string())?;
        if battery_intent.is_null() {
            return Err("battery intent is null".into());
        }

        // int level = intent.getIntExtra("level", -1);
        let key_level = env.new_string("level").map_err(|e| e.to_string())?;
        let level = env
            .call_method(
                &battery_intent,
                jni::jni_str!("getIntExtra"),
                jni::jni_sig!("(Ljava/lang/String;I)I"),
                &[JValue::Object(&key_level), JValue::Int(-1)],
            )
            .and_then(|v| v.i())
            .map_err(|e| e.to_string())?;

        // int scale = intent.getIntExtra("scale", -1);
        let key_scale = env.new_string("scale").map_err(|e| e.to_string())?;
        let scale = env
            .call_method(
                &battery_intent,
                jni::jni_str!("getIntExtra"),
                jni::jni_sig!("(Ljava/lang/String;I)I"),
                &[JValue::Object(&key_scale), JValue::Int(-1)],
            )
            .and_then(|v| v.i())
            .map_err(|e| e.to_string())?;

        // int status = intent.getIntExtra("status", -1);
        let key_status = env.new_string("status").map_err(|e| e.to_string())?;
        let status = env
            .call_method(
                &battery_intent,
                jni::jni_str!("getIntExtra"),
                jni::jni_sig!("(Ljava/lang/String;I)I"),
                &[JValue::Object(&key_status), JValue::Int(-1)],
            )
            .and_then(|v| v.i())
            .map_err(|e| e.to_string())?;

        Ok(Some((level, scale, status)))
        })();
        if result.is_err() { env.exception_clear(); }
        result
    })
    .unwrap_or(None)
}
