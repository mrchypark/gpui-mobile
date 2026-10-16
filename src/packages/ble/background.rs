use super::*;

/// Owns only this BLE session's native background context, never GPS service state.
pub(super) struct Lease {
    #[cfg(target_os = "android")]
    token: i64,
}
impl Lease {
    pub async fn acquire() -> Result<Self> {
        #[cfg(target_os = "android")]
        {
            let token = android_call("start", 0)?;
            let lease = Self { token };
            deadline(Duration::from_secs(3), async {
                loop {
                    if android_call("status", token)? == 1 { return Ok(()); }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }).await?;
            return Ok(lease);
        }
        #[cfg(target_os = "ios")]
        {
            use objc2::{class, msg_send, runtime::AnyObject};
            // UIBackgroundModes is a host entitlement declaration; it does not
            // create a restoration identifier or keep arbitrary worker code alive.
            objc2::rc::autoreleasepool(|_| unsafe {
                let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
                let key = crate::ios::util::nsstring("UIBackgroundModes");
                let modes: *mut AnyObject = msg_send![bundle, objectForInfoDictionaryKey: key];
                let valid: bool = !modes.is_null() && msg_send![modes, isKindOfClass: class!(NSArray)];
                let enabled: bool = valid && msg_send![modes, containsObject: crate::ios::util::nsstring("bluetooth-central")];
                if !enabled { return Err(Error::NotSupported("Declare UIBackgroundModes bluetooth-central before enabling background BLE".into())); }
                Ok(())
            })?;
            return Ok(Self {});
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        Err(Error::NotSupported("Mobile background BLE is only available on Android/iOS".into()))
    }
    pub fn check(&self) -> Result<()> {
        #[cfg(target_os = "android")]
        if android_call("status", self.token)? != 1 {
            return Err(Error::NotSupported("BLE foreground service is not running".into()));
        }
        Ok(())
    }
    pub fn release(&mut self) -> Result<()> {
        #[cfg(target_os = "android")]
        if self.token != 0 { android_call("stop", self.token)?; self.token = 0; }
        Ok(())
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.release().is_err() {
            eprintln!("BLE background service stop failed");
        }
    }
}
#[cfg(target_os = "android")]
fn android_call(operation: &str, token: i64) -> Result<i64> {
    use crate::android::jni as host;
    use jni::objects::JValue;
    static CLASS: std::sync::OnceLock<jni::refs::Global<jni::objects::JClass<'static>>> = std::sync::OnceLock::new();
    host::with_env(|env| {
        if CLASS.get().is_none() {
            if operation != "start" { return Err("BLE service class was not initialized".into()); }
            let class = host::find_app_class(env, "org.gpui.mobile.ble.BleConnectionService")?;
            let global = env.new_global_ref(class).map_err(|e| e.to_string())?;
            let _ = CLASS.set(global);
        }
        // Retain the application class, not an Activity: status/stop must work
        // after the UI owner is destroyed while the foreground service survives.
        let class = &**CLASS.get().unwrap();
        let result = match operation {
            "start" => {
                let activity = host::activity(env)?;
                env.call_static_method(class, jni::jni_str!("start"),
                    jni::jni_sig!("(Landroid/app/Activity;)J"), &[JValue::Object(&activity)])
                    .and_then(|v| v.j())
            }
            "status" => env.call_static_method(class, jni::jni_str!("status"),
                jni::jni_sig!("(J)I"), &[JValue::Long(token)]).and_then(|v| v.i()).map(i64::from),
            _ => env.call_static_method(class, jni::jni_str!("stop"),
                jni::jni_sig!("(J)V"), &[JValue::Long(token)]).map(|_| 0),
        };
        result.map_err(|e| { env.exception_clear(); e.to_string() })
    }).map_err(Error::RuntimeError)
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    #[test]
    fn unsupported_host_cannot_claim_mobile_background_context() {
        tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
            assert!(matches!(super::Lease::acquire().await, Err(super::Error::NotSupported(_))));
        });
    }
}
