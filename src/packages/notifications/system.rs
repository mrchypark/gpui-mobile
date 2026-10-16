use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use gpui::{SystemNotification, SystemNotificationResponse};
use std::sync::{Mutex, OnceLock};

type ResponseChannel = (
    UnboundedSender<SystemNotificationResponse>,
    Option<UnboundedReceiver<SystemNotificationResponse>>,
);
fn channel() -> &'static Mutex<ResponseChannel> {
    static CHANNEL: OnceLock<Mutex<ResponseChannel>> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        let (sender, receiver) = mpsc::unbounded();
        Mutex::new((sender, Some(receiver)))
    })
}

pub(super) fn enqueue(tag: String, action_id: Option<String>) {
    if let Ok(channel) = channel().lock() {
        let _ = channel.0.unbounded_send(SystemNotificationResponse {
            tag: tag.into(),
            action_id: action_id.map(Into::into),
        });
    }
}

pub(crate) fn take_system_notification_responses(
) -> Result<UnboundedReceiver<SystemNotificationResponse>, String> {
    #[cfg(target_os = "ios")]
    super::ios::install_system_delegate()?;
    poll_system_notification_responses()?;
    channel()
        .lock()
        .map_err(|_| "Notification response channel poisoned")?
        .1
        .take()
        .ok_or_else(|| "Notification response receiver already taken".into())
}

pub(crate) fn poll_system_notification_responses() -> Result<(), String> {
    #[cfg(target_os = "android")]
    return super::android::poll_system_responses();
    #[cfg(not(target_os = "android"))]
    Ok(())
}

pub(crate) fn show_system_notification(notification: &SystemNotification) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for action in &notification.actions {
        if !ids.insert(action.id.as_ref()) {
            return Err("Duplicate notification action id".into());
        }
    }
    #[cfg(target_os = "ios")]
    return super::ios::show_system(notification);
    #[cfg(target_os = "android")]
    return super::android::show_system(notification);
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    Err("System notifications require iOS or Android".into())
}

pub(crate) fn dismiss_system_notification(tag: &str) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    return super::ios::cancel_identifier(tag);
    #[cfg(target_os = "android")]
    return super::android::cancel_system(tag);
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = tag;
        Err("System notifications require iOS or Android".into())
    }
}
