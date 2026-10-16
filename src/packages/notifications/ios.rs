use super::Notification;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};

#[path = "../permission_handler/ios_completion.rs"]
mod completion;

#[link(name = "UserNotifications", kind = "framework")]
extern "C" {
    static UNNotificationDefaultActionIdentifier: *mut AnyObject;
    static UNNotificationDismissActionIdentifier: *mut AnyObject;
}

/// Get the shared UNUserNotificationCenter instance.
unsafe fn notification_center() -> *mut AnyObject {
    msg_send![class!(UNUserNotificationCenter), currentNotificationCenter]
}

pub fn initialize() -> Result<(), String> {
    unsafe {
        let center = notification_center();
        if center.is_null() {
            return Err("UNUserNotificationCenter not available".into());
        }

        // Request authorization: alert | sound | badge = 0x04 | 0x01 | 0x02 = 7
        let options: u64 = 7;

        let (tx, rx) = std::sync::mpsc::channel();
        let block = block2::RcBlock::new(move |granted: Bool, error: *mut AnyObject| {
            let result = if !error.is_null() {
                Err(completion::error_message(error))
            } else if granted.as_bool() {
                Ok(())
            } else {
                Err("Notification authorization denied".into())
            };
            let _ = tx.send(result);
        });
        let _: () = msg_send![
            center,
            requestAuthorizationWithOptions: options,
            completionHandler: &*block
        ];

        completion::wait(&rx)?
    }
}

pub fn show(notification: &Notification) -> Result<(), String> {
    show_identifier(notification, &notification.id.to_string(), None)
}

fn show_identifier(
    notification: &Notification,
    id: &str,
    category: Option<&str>,
) -> Result<(), String> {
    unsafe {
        let center = notification_center();
        if center.is_null() {
            return Err("UNUserNotificationCenter not available".into());
        }

        // Create UNMutableNotificationContent
        let content: *mut AnyObject = msg_send![class!(UNMutableNotificationContent), alloc];
        let content: *mut AnyObject = msg_send![content, init];
        if content.is_null() {
            return Err("Failed to create UNMutableNotificationContent".into());
        }

        // Set title
        let title = nsstring(&notification.title);
        let _: () = msg_send![content, setTitle: title];

        // Set body
        let body = nsstring(&notification.body);
        let _: () = msg_send![content, setBody: body];
        if let Some(category) = category {
            let _: () = msg_send![content, setCategoryIdentifier: nsstring(category)];
        }

        // Set sound to default
        let default_sound: *mut AnyObject = msg_send![class!(UNNotificationSound), defaultSound];
        let _: () = msg_send![content, setSound: default_sound];

        // Set userInfo with payload if present
        if let Some(ref payload) = notification.payload {
            let payload_str = nsstring(payload);
            let key = nsstring("payload");
            let user_info: *mut AnyObject = msg_send![
                class!(NSDictionary),
                dictionaryWithObject: payload_str,
                forKey: key
            ];
            let _: () = msg_send![content, setUserInfo: user_info];
        }

        // Create a UNNotificationRequest with the notification ID as identifier
        let identifier = nsstring(id);
        let null_trigger: *mut AnyObject = std::ptr::null_mut();
        let request: *mut AnyObject = msg_send![
            class!(UNNotificationRequest),
            requestWithIdentifier: identifier,
            content: content,
            trigger: null_trigger
        ];
        let _: () = msg_send![content, release];

        if request.is_null() {
            return Err("Failed to create UNNotificationRequest".into());
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let block = block2::RcBlock::new(move |error: *mut AnyObject| {
            let result = if error.is_null() {
                Ok(())
            } else {
                Err(completion::error_message(error))
            };
            let _ = tx.send(result);
        });
        let _: () = msg_send![
            center,
            addNotificationRequest: request,
            withCompletionHandler: &*block
        ];

        completion::wait(&rx)?
    }
}

pub fn cancel(id: i32) -> Result<(), String> {
    cancel_identifier(&id.to_string())
}

pub(super) fn cancel_identifier(id: &str) -> Result<(), String> {
    unsafe {
        let center = notification_center();
        if center.is_null() {
            return Err("UNUserNotificationCenter not available".into());
        }

        let identifier = nsstring(id);
        let array: *mut AnyObject = msg_send![
            class!(NSArray),
            arrayWithObject: identifier
        ];

        // Remove both pending and delivered notifications
        let _: () = msg_send![
            center,
            removePendingNotificationRequestsWithIdentifiers: array
        ];
        let _: () = msg_send![
            center,
            removeDeliveredNotificationsWithIdentifiers: array
        ];

        Ok(())
    }
}

pub fn cancel_all() -> Result<(), String> {
    unsafe {
        let center = notification_center();
        if center.is_null() {
            return Err("UNUserNotificationCenter not available".into());
        }

        let _: () = msg_send![center, removeAllPendingNotificationRequests];
        let _: () = msg_send![center, removeAllDeliveredNotifications];

        Ok(())
    }
}

use crate::ios::util::nsstring;

#[path = "system_identity.rs"]
mod identity;

unsafe fn string(object: *mut AnyObject) -> String {
    if object.is_null() {
        return String::new();
    }
    let bytes: *const std::ffi::c_char = msg_send![object, UTF8String];
    let length: usize = msg_send![object, lengthOfBytesUsingEncoding: 4usize];
    if bytes.is_null() {
        String::new()
    } else {
        String::from_utf8_lossy(std::slice::from_raw_parts(bytes.cast::<u8>(), length)).into_owned()
    }
}

unsafe extern "C" fn did_receive(
    _: *mut AnyObject,
    _: objc2::runtime::Sel,
    _: *mut AnyObject,
    response: *mut AnyObject,
    done: *const block2::DynBlock<dyn Fn()>,
) {
    let notification: *mut AnyObject = msg_send![response, notification];
    let request: *mut AnyObject = msg_send![notification, request];
    let tag: *mut AnyObject = msg_send![request, identifier];
    let action: *mut AnyObject = msg_send![response, actionIdentifier];
    let action = string(action);
    let decoded = if action == string(UNNotificationDefaultActionIdentifier) {
        Some(None)
    } else if action == string(UNNotificationDismissActionIdentifier) {
        None
    } else {
        identity::response_action(&action)
    };
    if let Some(action_id) = decoded {
        super::system::enqueue(string(tag), action_id);
    }
    // Native completion is nonnull and valid for this callback; do not retain it.
    (*done).call(());
}

unsafe extern "C" fn will_present(
    _: *mut AnyObject,
    _: objc2::runtime::Sel,
    _: *mut AnyObject,
    _: *mut AnyObject,
    done: *const block2::DynBlock<dyn Fn(usize)>,
) {
    (*done).call((2 | 8 | 16,)); // Sound, list, banner. Never requests authorization.
}

pub(super) fn install_system_delegate() -> Result<(), String> {
    use objc2::runtime::{ClassBuilder, Sel};
    use std::sync::{Mutex, OnceLock};
    static DELEGATE: OnceLock<Mutex<usize>> = OnceLock::new();
    let mut delegate = DELEGATE
        .get_or_init(|| Mutex::new(0))
        .lock()
        .map_err(|_| "Notification delegate lock poisoned")?;
    unsafe {
        let center = notification_center();
        if center.is_null() {
            return Err("UNUserNotificationCenter unavailable".into());
        }
        let existing: *mut AnyObject = msg_send![center, delegate];
        if !existing.is_null() && existing as usize != *delegate {
            return Err("UNUserNotificationCenter already has another delegate".into());
        }
        if *delegate == 0 {
            let mut builder =
                ClassBuilder::new(c"GpuiSystemNotificationDelegate", class!(NSObject))
                    .ok_or("Notification delegate class already registered")?;
            builder.add_method(objc2::sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
                did_receive as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *const block2::DynBlock<dyn Fn()>));
            builder.add_method(
                objc2::sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:),
                will_present
                    as unsafe extern "C" fn(
                        *mut AnyObject,
                        Sel,
                        *mut AnyObject,
                        *mut AnyObject,
                        *const block2::DynBlock<dyn Fn(usize)>,
                    ),
            );
            let object: *mut AnyObject = msg_send![builder.register(), new];
            if object.is_null() {
                return Err("Cannot allocate notification delegate".into());
            }
            *delegate = object as usize; // Center's delegate is weak; application-lifetime owner.
        }
        let _: () = msg_send![center, setDelegate: *delegate as *mut AnyObject];
    }
    Ok(())
}

pub(super) fn show_system(notification: &gpui::SystemNotification) -> Result<(), String> {
    static REGISTER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = REGISTER
        .try_lock()
        .map_err(|_| "Another system notification is being registered")?;
    install_system_delegate()?;
    let category_id = format!("gpui-system:{}", notification.tag);
    unsafe {
        let actions: *mut AnyObject = msg_send![class!(NSMutableArray), array];
        for action in &notification.actions {
            let action: *mut AnyObject = msg_send![class!(UNNotificationAction),
                actionWithIdentifier: nsstring(&identity::native_action(&action.id)),
                title: nsstring(&action.label), options: 4usize];
            let _: () = msg_send![actions, addObject: action];
        }
        let empty: *mut AnyObject = msg_send![class!(NSArray), array];
        let category: *mut AnyObject = msg_send![class!(UNNotificationCategory),
            categoryWithIdentifier: nsstring(&category_id), actions: actions,
            intentIdentifiers: empty, options: 0usize];
        let _: () = msg_send![category, retain];
        let category_address = category as usize;
        let center_address = notification_center() as usize;
        let category_name = category_id.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let block = block2::RcBlock::new(move |categories: *mut AnyObject| {
            let merged: *mut AnyObject = msg_send![categories, mutableCopy];
            let objects: *mut AnyObject = msg_send![categories, allObjects];
            let count: usize = msg_send![objects, count];
            for index in 0..count {
                let old: *mut AnyObject = msg_send![objects, objectAtIndex: index];
                let identifier: *mut AnyObject = msg_send![old, identifier];
                if string(identifier) == category_name {
                    let _: () = msg_send![merged, removeObject: old];
                }
            }
            let category = category_address as *mut AnyObject;
            let _: () = msg_send![merged, addObject: category];
            let _: () =
                msg_send![center_address as *mut AnyObject, setNotificationCategories: merged];
            let _: () = msg_send![merged, release];
            let _: () = msg_send![category, release];
            let _ = tx.send(());
        });
        let _: () = msg_send![notification_center(), getNotificationCategoriesWithCompletionHandler: &*block];
        completion::wait(&rx)?;
    }
    show_identifier(
        &Notification {
            id: 0,
            title: notification.title.to_string(),
            body: notification.body.to_string(),
            channel: Default::default(),
            payload: None,
        },
        &notification.tag,
        Some(&category_id),
    )
}
