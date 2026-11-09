use crate::android::jni as app_jni;
use crate::android::jni::JniExt;
use std::path::PathBuf;

pub fn temporary_directory() -> Result<PathBuf, String> {
    // Use cache dir + "tmp" subdirectory
    super::directories::temporary_directory(cache_directory()?)
}

pub fn documents_directory() -> Result<PathBuf, String> {
    // AndroidApp::internal_data_path() maps to Context.getFilesDir()
    let app = app_jni::android_app().ok_or_else(|| "AndroidApp not initialised".to_string())?;
    app.internal_data_path()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| "internal_data_path() returned None".into())
}

pub fn cache_directory() -> Result<PathBuf, String> {
    app_jni::with_env(|env| {
        let activity = app_jni::activity(env)?;
        let directory = env
            .call_method(
                &activity,
                jni::jni_str!("getCacheDir"),
                jni::jni_sig!("()Ljava/io/File;"),
                &[],
            )
            .and_then(|v| v.l())
            .e()?;
        if directory.is_null() {
            return Err("Context.getCacheDir returned null".into());
        }
        let path = env
            .call_method(
                &directory,
                jni::jni_str!("getAbsolutePath"),
                jni::jni_sig!("()Ljava/lang/String;"),
                &[],
            )
            .and_then(|v| v.l())
            .e()?;
        if path.is_null() {
            return Err("Cache directory path is null".into());
        }
        let path = app_jni::get_string(env, &path);
        if path.is_empty() {
            return Err("Cache directory path is empty".into());
        }
        Ok(PathBuf::from(path))
    })
}

pub fn support_directory() -> Result<PathBuf, String> {
    // On Android, getFilesDir() is the closest equivalent to Application Support.
    documents_directory()
}
