use super::{DirectoryDocument, OpenFileOptions, SaveFileOptions, SelectedFile};
use crate::android::jni::{self as jni_helpers, JniExt};
use jni::objects::{JClass, JObject, JValue};

const HELPER_CLASS: &str = "dev.gpui.mobile.GpuiFilePicker";

pub fn open_file(options: &OpenFileOptions) -> Result<Option<SelectedFile>, String> {
    let mime_types = build_mime_string(options);
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        let j_mime = env.new_string(&mime_types).e()?;

        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("openFile"),
                jni::jni_sig!("(Landroid/app/Activity;Ljava/lang/String;)Ljava/lang/String;"),
                &[JValue::Object(&activity), JValue::Object(&j_mime)],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;

        if result.is_null() {
            return Ok(None);
        }

        let path = jni_helpers::get_string(env, &result);
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        Ok(Some(SelectedFile { path, name }))
    })
}

pub fn open_files(options: &OpenFileOptions) -> Result<Vec<SelectedFile>, String> {
    let mime_types = build_mime_string(options);
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        let j_mime = env.new_string(&mime_types).e()?;

        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("openFiles"),
                jni::jni_sig!("(Landroid/app/Activity;Ljava/lang/String;)[Ljava/lang/String;"),
                &[JValue::Object(&activity), JValue::Object(&j_mime)],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;

        if result.is_null() {
            return Ok(vec![]);
        }

        let arr = unsafe { jni::objects::JObjectArray::<JObject>::from_raw(env, result.as_raw()) };
        let len = arr.len(env).e()?;
        let mut files = Vec::with_capacity(len);
        for i in 0..len {
            let obj: JObject = arr.get_element(env, i).e()?;
            let path = jni_helpers::get_string(env, &obj);
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            files.push(SelectedFile { path, name });
        }
        Ok(files)
    })
}

pub fn get_save_path(options: &SaveFileOptions) -> Result<Option<String>, String> {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        let mime_types = build_mime_string_from_save(options);
        let j_mime = env.new_string(&mime_types).e()?;
        let j_name = env
            .new_string(options.suggested_name.as_deref().unwrap_or("untitled"))
            .e()?;

        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("getSavePath"),
                jni::jni_sig!("(Landroid/app/Activity;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
                &[
                    JValue::Object(&activity),
                    JValue::Object(&j_mime),
                    JValue::Object(&j_name),
                ],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;

        if result.is_null() {
            return Ok(None);
        }

        let path = jni_helpers::get_string(env, &result);
        Ok(Some(path))
    })
}

pub fn export_file(
    options: &SaveFileOptions,
    contents: &[u8],
) -> Result<Option<SelectedFile>, String> {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        let mime_types = build_mime_string_from_save(options);
        let j_mime = env.new_string(&mime_types).e()?;
        let j_name = env
            .new_string(options.suggested_name.as_deref().unwrap_or("untitled"))
            .e()?;
        let j_contents = env.byte_array_from_slice(contents).e()?;

        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("exportFile"),
                jni::jni_sig!("(Landroid/app/Activity;Ljava/lang/String;Ljava/lang/String;[B)Ljava/lang/String;"),
                &[
                    JValue::Object(&activity),
                    JValue::Object(&j_mime),
                    JValue::Object(&j_name),
                    JValue::Object(&j_contents),
                ],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;

        if result.is_null() {
            return Ok(None);
        }

        let path = jni_helpers::get_string(env, &result);
        // Keep a successful export successful even when the provider cannot
        // expose metadata. Do not substitute the URI document ID or the
        // suggested name for a provider display name.
        let provider_name = provider_display_name(env, &cls, &activity, &path);
        let name = exported_name(provider_name);
        Ok(Some(SelectedFile { path, name }))
    })
}

fn provider_display_name(
    env: &mut jni::Env<'_>,
    helper: &JClass<'_>,
    activity: &JObject<'_>,
    path: &str,
) -> Option<String> {
    let uri = match env.new_string(path) {
        Ok(uri) => uri,
        Err(_) => {
            env.exception_clear();
            return None;
        }
    };
    let value = match env.call_static_method(
        helper,
        jni::jni_str!("displayName"),
        jni::jni_sig!("(Landroid/app/Activity;Ljava/lang/String;)Ljava/lang/String;"),
        &[JValue::Object(activity), JValue::Object(&uri)],
    ) {
        Ok(value) => match value.l() {
            Ok(value) => value,
            Err(_) => {
                env.exception_clear();
                return None;
            }
        },
        Err(_) => {
            env.exception_clear();
            return None;
        }
    };
    if value.is_null() {
        return None;
    }
    let name = jni_helpers::get_string(env, &value);
    if name.trim().is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Use only provider metadata. An unavailable name stays empty so callers do
/// not receive the URI document ID or a possibly renamed suggested name.
fn exported_name(provider: Option<String>) -> String {
    provider
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_default()
}

pub fn get_directory_path(_initial_directory: Option<&str>) -> Result<Option<String>, String> {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("getDirectoryPath"),
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

        let path = jni_helpers::get_string(env, &result);
        Ok(Some(path))
    })
}

pub fn list_directory(tree_uri: &str) -> Result<Vec<DirectoryDocument>, String> {
    if tree_uri.trim().is_empty() {
        return Err("Directory tree URI cannot be empty".into());
    }
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let j_uri = env.new_string(tree_uri).e()?;
        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("listDirectory"),
                jni::jni_sig!("(Landroid/app/Activity;Ljava/lang/String;)[Ljava/lang/String;"),
                &[JValue::Object(&activity), JValue::Object(&j_uri)],
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
        let mut docs = Vec::with_capacity(len / 5);
        for base in (0..len).step_by(5) {
            if base + 4 >= len {
                break;
            }
            let mut get = |idx| -> Result<String, String> {
                let obj: JObject = arr.get_element(env, idx).e()?;
                Ok(jni_helpers::get_string(env, &obj))
            };
            let uri = get(base)?;
            let name = get(base + 1)?;
            let mime = get(base + 2)?;
            let size = get(base + 3)?;
            let modified = get(base + 4)?;
            docs.push(DirectoryDocument {
                uri,
                name,
                mime_type: (!mime.is_empty()).then_some(mime),
                size_bytes: (!size.is_empty())
                    .then(|| size.parse::<u64>().ok())
                    .flatten(),
                modified_ms: (!modified.is_empty())
                    .then(|| modified.parse::<u64>().ok())
                    .flatten(),
            });
        }
        Ok(docs)
    })
}

pub fn import_document(document_uri: &str) -> Result<SelectedFile, String> {
    if document_uri.trim().is_empty() {
        return Err("Document URI cannot be empty".into());
    }
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let j_uri = env.new_string(document_uri).e()?;
        let result = env
            .call_static_method(
                &cls,
                jni::jni_str!("importDocument"),
                jni::jni_sig!("(Landroid/app/Activity;Ljava/lang/String;)[Ljava/lang/String;"),
                &[JValue::Object(&activity), JValue::Object(&j_uri)],
            )
            .and_then(|v| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;
        if result.is_null() {
            return Err("Failed to import document".into());
        }
        let arr = unsafe { jni::objects::JObjectArray::<JObject>::from_raw(env, result.as_raw()) };
        let len = arr.len(env).e()?;
        if len < 2 {
            return Err("Invalid imported document result".into());
        }
        let path_obj: JObject = arr.get_element(env, 0).e()?;
        let name_obj: JObject = arr.get_element(env, 1).e()?;
        Ok(SelectedFile {
            path: jni_helpers::get_string(env, &path_obj),
            name: jni_helpers::get_string(env, &name_obj),
        })
    })
}

fn build_mime_string(options: &OpenFileOptions) -> String {
    if options.accept_type_groups.is_empty() {
        return "*/*".to_string();
    }
    let mut mimes: Vec<String> = Vec::new();
    for group in &options.accept_type_groups {
        if !group.mime_types.is_empty() {
            mimes.extend(group.mime_types.iter().cloned());
        } else if !group.extensions.is_empty() {
            for ext in &group.extensions {
                mimes.push(extension_to_mime(ext));
            }
        } else {
            mimes.push("*/*".to_string());
        }
    }
    if mimes.is_empty() {
        "*/*".to_string()
    } else {
        mimes.join("|")
    }
}

fn build_mime_string_from_save(options: &SaveFileOptions) -> String {
    if options.accept_type_groups.is_empty() {
        return "*/*".to_string();
    }
    let mut mimes: Vec<String> = Vec::new();
    for group in &options.accept_type_groups {
        if !group.mime_types.is_empty() {
            mimes.extend(group.mime_types.iter().cloned());
        } else if !group.extensions.is_empty() {
            for ext in &group.extensions {
                mimes.push(extension_to_mime(ext));
            }
        }
    }
    mimes.first().cloned().unwrap_or_else(|| "*/*".to_string())
}

fn extension_to_mime(ext: &str) -> String {
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "csv" => "text/csv",
        _ => "application/octet-stream",
    }
    .to_string()
}
