use super::PackageInfo;
use crate::android::jni::{self as jni_helpers, get_string, JniExt};
use jni::objects::JValue;

pub fn get_package_info() -> Result<PackageInfo, String> {
    jni_helpers::with_env(|env| {
        let result = (|| {
            let activity = jni_helpers::activity(env)?;
            let package = env
                .call_method(
                    &activity,
                    jni::jni_str!("getPackageName"),
                    jni::jni_sig!("()Ljava/lang/String;"),
                    &[],
                )
                .and_then(|v| v.l())
                .e()?;
            let package_name = get_string(env, &package);
            if package_name.is_empty() {
                return Err("Package name unavailable".into());
            }
            let manager = env
                .call_method(
                    &activity,
                    jni::jni_str!("getPackageManager"),
                    jni::jni_sig!("()Landroid/content/pm/PackageManager;"),
                    &[],
                )
                .and_then(|v| v.l())
                .e()?;
            let info = env
                .call_method(
                    &manager,
                    jni::jni_str!("getPackageInfo"),
                    jni::jni_sig!("(Ljava/lang/String;I)Landroid/content/pm/PackageInfo;"),
                    &[JValue::Object(&package), JValue::Int(0)],
                )
                .and_then(|v| v.l())
                .e()?;
            let version = env
                .get_field(
                    &info,
                    jni::jni_str!("versionName"),
                    jni::jni_sig!("Ljava/lang/String;"),
                )
                .and_then(|v| v.l())
                .e()?;
            let version = get_string(env, &version);
            let sdk = env
                .get_static_field(
                    jni::jni_str!("android/os/Build$VERSION"),
                    jni::jni_str!("SDK_INT"),
                    jni::jni_sig!("I"),
                )
                .and_then(|v| v.i())
                .e()?;
            let build_number = if sdk >= 28 {
                env.call_method(
                    &info,
                    jni::jni_str!("getLongVersionCode"),
                    jni::jni_sig!("()J"),
                    &[],
                )
                .and_then(|v| v.j())
                .e()?
                .to_string()
            } else {
                env.get_field(&info, jni::jni_str!("versionCode"), jni::jni_sig!("I"))
                    .and_then(|v| v.i())
                    .e()?
                    .to_string()
            };
            let application = env
                .call_method(
                    &activity,
                    jni::jni_str!("getApplicationInfo"),
                    jni::jni_sig!("()Landroid/content/pm/ApplicationInfo;"),
                    &[],
                )
                .and_then(|v| v.l())
                .e()?;
            let label = env
                .call_method(
                    &manager,
                    jni::jni_str!("getApplicationLabel"),
                    jni::jni_sig!("(Landroid/content/pm/ApplicationInfo;)Ljava/lang/CharSequence;"),
                    &[JValue::Object(&application)],
                )
                .and_then(|v| v.l())
                .e()?;
            let label = env
                .call_method(
                    &label,
                    jni::jni_str!("toString"),
                    jni::jni_sig!("()Ljava/lang/String;"),
                    &[],
                )
                .and_then(|v| v.l())
                .e()?;
            Ok(PackageInfo {
                package_name,
                version,
                build_number,
                app_name: get_string(env, &label),
            })
        })();
        if result.is_err() {
            env.exception_clear();
        }
        result
    })
}
