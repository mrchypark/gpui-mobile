use super::Notification;
use crate::android::jni::{self as jni_helpers, JniExt};
use jni::objects::JValue;

const HELPER_CLASS: &str = "dev.gpui.mobile.GpuiNotifications";

pub fn initialize() -> Result<(), String> {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        env.call_static_method(
            &cls,
            jni::jni_str!("initialize"),
            jni::jni_sig!("(Landroid/app/Activity;)V"),
            &[JValue::Object(&activity)],
        )
        .map_err(|e| {
            env.exception_clear();
            e.to_string()
        })?;

        Ok(())
    })
}

pub fn show(notification: &Notification) -> Result<(), String> {
    #[cfg(feature = "permission_handler")]
    {
        let status = crate::packages::permission_handler::check_permission(
            crate::packages::permission_handler::Permission::Notification,
        )?;
        if !status.is_granted() {
            return Err("notification permission not granted".into());
        }
    }

    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        let j_title = env.new_string(&notification.title).e()?;
        let j_body = env.new_string(&notification.body).e()?;
        let j_channel_id = env.new_string(&notification.channel.id).e()?;
        let j_channel_name = env.new_string(&notification.channel.name).e()?;
        let j_channel_desc = env.new_string(&notification.channel.description).e()?;
        let j_payload = env
            .new_string(notification.payload.as_deref().unwrap_or(""))
            .e()?;

        let importance = notification.channel.importance.as_i32();

        env.call_static_method(
            &cls,
            jni::jni_str!("show"),
            jni::jni_sig!("(Landroid/app/Activity;ILjava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;ILjava/lang/String;)V"),
            &[
                JValue::Object(&activity),
                JValue::Int(notification.id),
                JValue::Object(&j_title),
                JValue::Object(&j_body),
                JValue::Object(&j_channel_id),
                JValue::Object(&j_channel_name),
                JValue::Object(&j_channel_desc),
                JValue::Int(importance),
                JValue::Object(&j_payload),
            ],
        )
        .map_err(|e| {
            env.exception_clear();
            e.to_string()
        })?;

        Ok(())
    })
}

pub fn cancel(id: i32) -> Result<(), String> {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        env.call_static_method(
            &cls,
            jni::jni_str!("cancel"),
            jni::jni_sig!("(Landroid/app/Activity;I)V"),
            &[JValue::Object(&activity), JValue::Int(id)],
        )
        .map_err(|e| {
            env.exception_clear();
            e.to_string()
        })?;

        Ok(())
    })
}

pub fn cancel_all() -> Result<(), String> {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let cls = jni_helpers::find_app_class(env, HELPER_CLASS)?;

        env.call_static_method(
            &cls,
            jni::jni_str!("cancelAll"),
            jni::jni_sig!("(Landroid/app/Activity;)V"),
            &[JValue::Object(&activity)],
        )
        .map_err(|e| {
            env.exception_clear();
            e.to_string()
        })?;

        Ok(())
    })
}

pub(super) fn show_system(notification: &gpui::SystemNotification) -> Result<(), String> {
    use jni::objects::{JObject, JObjectArray, JString};
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let class = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let tag = env.new_string(notification.tag.as_ref()).e()?;
        let title = env.new_string(notification.title.as_ref()).e()?;
        let body = env.new_string(notification.body.as_ref()).e()?;
        let ids =
            JObjectArray::<JString>::new(env, notification.actions.len(), &JString::null()).e()?;
        let labels =
            JObjectArray::<JString>::new(env, notification.actions.len(), &JString::null()).e()?;
        for (index, action) in notification.actions.iter().enumerate() {
            let id = env.new_string(action.id.as_ref()).e()?;
            let label = env.new_string(action.label.as_ref()).e()?;
            ids.set_element(env, index, &id).e()?;
            labels.set_element(env, index, &label).e()?;
        }
        env.call_static_method(&class, jni::jni_str!("showSystem"),
            jni::jni_sig!("(Landroid/app/Activity;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;[Ljava/lang/String;)V"),
            &[JValue::Object(&activity), JValue::Object(&tag), JValue::Object(&title), JValue::Object(&body),
                JValue::Object(ids.as_ref() as &JObject), JValue::Object(labels.as_ref() as &JObject)])
            .map_err(|error| { env.exception_clear(); error.to_string() })?;
        Ok(())
    })
}

pub(super) fn cancel_system(tag: &str) -> Result<(), String> {
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let class = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let tag = env.new_string(tag).e()?;
        env.call_static_method(
            &class,
            jni::jni_str!("cancelSystem"),
            jni::jni_sig!("(Landroid/app/Activity;Ljava/lang/String;)V"),
            &[JValue::Object(&activity), JValue::Object(&tag)],
        )
        .map_err(|error| {
            env.exception_clear();
            error.to_string()
        })?;
        Ok(())
    })
}

pub(super) fn poll_system_responses() -> Result<(), String> {
    use jni::objects::{JObject, JObjectArray};
    jni_helpers::with_env(|env| {
        let activity = jni_helpers::activity(env)?;
        let class = jni_helpers::find_app_class(env, HELPER_CLASS)?;
        let result = env
            .call_static_method(
                &class,
                jni::jni_str!("drainResponses"),
                jni::jni_sig!("(Landroid/app/Activity;)[Ljava/lang/String;"),
                &[JValue::Object(&activity)],
            )
            .and_then(|v| v.l())
            .map_err(|error| {
                env.exception_clear();
                error.to_string()
            })?;
        if result.is_null() {
            return Err("Missing notification response array".into());
        }
        let array = unsafe { JObjectArray::<JObject>::from_raw(env, result.into_raw()) };
        let len = array.len(env).e()?;
        if len % 3 != 0 {
            return Err("Invalid notification response array".into());
        }
        for index in (0..len).step_by(3) {
            let tag = array.get_element(env, index).e()?;
            let present = array.get_element(env, index + 1).e()?;
            let action = array.get_element(env, index + 2).e()?;
            super::system::enqueue(
                jni_helpers::get_string(env, &tag),
                if jni_helpers::get_string(env, &present) == "1" {
                    Some(jni_helpers::get_string(env, &action))
                } else {
                    None
                },
            );
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_dev_gpui_mobile_GpuiNotifications_nativeWake(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
) {
    if let Err(error) = poll_system_responses() {
        log::warn!("Notification response pending: {error}");
    }
}
