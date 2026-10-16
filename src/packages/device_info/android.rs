use super::DeviceInfo;

pub fn get_device_info() -> Result<DeviceInfo, String> {
    let model = get_system_property("ro.product.model");
    let manufacturer = get_system_property("ro.product.manufacturer");
    let device_name = device_name()?.unwrap_or_else(|| model.clone());
    let os_version = get_system_property("ro.build.version.release");
    let fingerprint = get_system_property("ro.build.fingerprint");

    if model.is_empty() || manufacturer.is_empty() || os_version.is_empty() {
        return Err("Required Android device properties are unavailable".into());
    }
    let is_physical_device = get_system_property("ro.kernel.qemu") != "1"
        && !fingerprint.contains("generic")
        && !fingerprint.contains("emulator")
        && !fingerprint.contains("sdk");

    Ok(DeviceInfo {
        model,
        manufacturer,
        os_version,
        device_name,
        is_physical_device,
    })
}

fn device_name() -> Result<Option<String>, String> {
    use crate::android::jni::{self, get_string, JniExt};
    use ::jni::objects::JValue;
    jni::with_env(|env| {
        let result = (|| {
            let activity = jni::activity(env)?;
            let resolver = env
                .call_method(
                    &activity,
                    ::jni::jni_str!("getContentResolver"),
                    ::jni::jni_sig!("()Landroid/content/ContentResolver;"),
                    &[],
                )
                .and_then(|v| v.l())
                .e()?;
            let key = env.new_string("device_name").e()?;
            let name = env
                .call_static_method(
                    ::jni::jni_str!("android/provider/Settings$Global"),
                    ::jni::jni_str!("getString"),
                    ::jni::jni_sig!(
                        "(Landroid/content/ContentResolver;Ljava/lang/String;)Ljava/lang/String;"
                    ),
                    &[JValue::Object(&resolver), JValue::Object(&key)],
                )
                .and_then(|v| v.l())
                .e()?;
            if name.is_null() {
                Ok(None)
            } else {
                Ok(Some(get_string(env, &name)))
            }
        })();
        if result.is_err() {
            env.exception_clear();
        }
        result
    })
}

/// Read an Android system property via the NDK C API.
fn get_system_property(name: &str) -> String {
    // Property name must be null-terminated
    let mut name_buf = Vec::with_capacity(name.len() + 1);
    name_buf.extend_from_slice(name.as_bytes());
    name_buf.push(0);

    // PROP_VALUE_MAX is 92 bytes
    let mut value_buf = [0u8; 92];

    unsafe {
        let len = __system_property_get(
            name_buf.as_ptr() as *const i8,
            value_buf.as_mut_ptr() as *mut i8,
        );
        if len > 0 {
            String::from_utf8_lossy(&value_buf[..len as usize]).into_owned()
        } else {
            String::new()
        }
    }
}

extern "C" {
    fn __system_property_get(name: *const i8, value: *mut i8) -> i32;
}
