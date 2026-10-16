use crate::android::jni::{self as jni_helpers, get_string};
use jni::objects::JValue;

const HELPER_CLASS: &str = "dev.gpui.mobile.GpuiDeepLink";

pub fn get_initial_link() -> Result<Option<String>, String> {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("getInitialLink"),
                jni::jni_sig!("(Landroid/app/Activity;)Ljava/lang/String;"),
                &[JValue::Object(&activity)],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;

        if result.is_null() {
            return Ok(None);
        }

        let url = get_string(env, &result);
        if url.is_empty() {
            Ok(None)
        } else {
            super::LATEST_LINK
                .lock()
                .unwrap()
                .get_or_insert_with(|| url.clone());
            Ok(Some(url))
        }
    })
}
