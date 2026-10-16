use super::*;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, sel};
use std::sync::mpsc;

#[path = "ios_bluetooth.rs"]
mod bluetooth;
#[path = "ios_completion.rs"]
mod completion;
#[path = "ios_location.rs"]
mod location;

#[link(name = "MediaPlayer", kind = "framework")]
extern "C" {}

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

#[link(name = "CoreLocation", kind = "framework")]
extern "C" {}

#[link(name = "Photos", kind = "framework")]
extern "C" {}

#[link(name = "Contacts", kind = "framework")]
extern "C" {}

#[link(name = "EventKit", kind = "framework")]
extern "C" {}

#[link(name = "UserNotifications", kind = "framework")]
extern "C" {}

#[link(name = "CoreBluetooth", kind = "framework")]
extern "C" {}

#[link(name = "CoreMotion", kind = "framework")]
extern "C" {}

#[link(name = "Speech", kind = "framework")]
extern "C" {}

#[link(name = "AppTrackingTransparency", kind = "framework")]
extern "C" {}

// ── Helpers ─────────────────────────────────────────────────────────────────

use crate::ios::util::nsstring;

// ── Check permission ────────────────────────────────────────────────────────

pub fn check_permission(permission: Permission) -> Result<PermissionStatus, String> {
    unsafe {
        match permission {
            Permission::Camera => check_av_authorization("vide"),
            Permission::Microphone => check_av_authorization("soun"),
            Permission::Photos => check_photos_authorization(),
            Permission::LocationWhenInUse => location::check(false),
            Permission::LocationAlways => location::check(true),
            Permission::Contacts => check_contacts_authorization(),
            Permission::Calendar => check_event_authorization(0), // EKEntityTypeEvent
            Permission::Reminders => check_event_authorization(1), // EKEntityTypeReminder
            Permission::Notification => check_notification_authorization(),
            Permission::Bluetooth => check_bluetooth_authorization(),
            Permission::Speech => check_speech_authorization(),
            Permission::AppTrackingTransparency => check_tracking_authorization(),
            Permission::Sensors => Err("iOS exposes no authorization status/request for raw accelerometer and gyroscope; Motion & Fitness is a different permission scope".into()),
            Permission::MediaLibrary => {
                let status: i64 = msg_send![class!(MPMediaLibrary), authorizationStatus];
                Ok(super::ios_status::media_library(status))
            }
            // Android-only permissions
            Permission::Storage
            | Permission::SystemAlertWindow
            | Permission::InstallPackages
            | Permission::AccessNotificationPolicy
            | Permission::Phone
            | Permission::Sms
            | Permission::Audio => Err("This permission is not applicable on iOS".into()),
            Permission::Videos => check_photos_authorization(), // iOS uses Photos framework for videos too
        }
    }
}

unsafe fn check_av_authorization(media_type: &str) -> Result<PermissionStatus, String> {
    let ns_media = nsstring(media_type);
    let status: i64 = msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: ns_media];
    Ok(av_status_to_permission(status))
}

fn av_status_to_permission(status: i64) -> PermissionStatus {
    match status {
        0 => PermissionStatus::Denied, // AVAuthorizationStatusNotDetermined
        1 => PermissionStatus::Restricted, // AVAuthorizationStatusRestricted
        2 => PermissionStatus::PermanentlyDenied, // AVAuthorizationStatusDenied
        3 => PermissionStatus::Granted, // AVAuthorizationStatusAuthorized
        _ => PermissionStatus::Denied,
    }
}

unsafe fn check_photos_authorization() -> Result<PermissionStatus, String> {
    // PHAuthorizationStatus
    let status: i64 = msg_send![class!(PHPhotoLibrary), authorizationStatusForAccessLevel: 1i64];
    Ok(match status {
        0 => PermissionStatus::Denied, // PHAuthorizationStatusNotDetermined
        1 => PermissionStatus::Restricted, // PHAuthorizationStatusRestricted
        2 => PermissionStatus::PermanentlyDenied, // PHAuthorizationStatusDenied
        3 => PermissionStatus::Granted, // PHAuthorizationStatusAuthorized
        4 => PermissionStatus::Limited, // PHAuthorizationStatusLimited
        _ => PermissionStatus::Denied,
    })
}

unsafe fn check_contacts_authorization() -> Result<PermissionStatus, String> {
    let status: i64 = msg_send![class!(CNContactStore), authorizationStatusForEntityType: 0i64]; // CNEntityTypeContacts
    Ok(match status {
        0 => PermissionStatus::Denied, // CNAuthorizationStatusNotDetermined
        1 => PermissionStatus::Restricted, // CNAuthorizationStatusRestricted
        2 => PermissionStatus::PermanentlyDenied, // CNAuthorizationStatusDenied
        3 => PermissionStatus::Granted, // CNAuthorizationStatusAuthorized
        4 => PermissionStatus::Limited, // CNAuthorizationStatusLimited (iOS 18+)
        _ => PermissionStatus::Denied,
    })
}

unsafe fn check_event_authorization(entity_type: i64) -> Result<PermissionStatus, String> {
    let status: i64 =
        msg_send![class!(EKEventStore), authorizationStatusForEntityType: entity_type];
    Ok(match status {
        0 => PermissionStatus::Denied, // EKAuthorizationStatusNotDetermined
        1 => PermissionStatus::Restricted, // EKAuthorizationStatusRestricted
        2 => PermissionStatus::PermanentlyDenied, // EKAuthorizationStatusDenied
        3 => PermissionStatus::Granted, // EKAuthorizationStatusAuthorized
        4 => PermissionStatus::Limited, // EKAuthorizationStatusWriteOnly
        _ => PermissionStatus::Denied,
    })
}

unsafe fn check_notification_authorization() -> Result<PermissionStatus, String> {
    // UNUserNotificationCenter is async, so we use a sync channel
    let (tx, rx) = mpsc::channel();

    let center: *mut AnyObject =
        msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
    let block = block2::RcBlock::new(move |settings: *mut AnyObject| {
        let auth_status: i64 = unsafe { msg_send![settings, authorizationStatus] };
        let status = match auth_status {
            0 => PermissionStatus::Denied, // UNAuthorizationStatusNotDetermined
            1 => PermissionStatus::PermanentlyDenied, // UNAuthorizationStatusDenied
            2 => PermissionStatus::Granted, // UNAuthorizationStatusAuthorized
            3 => PermissionStatus::Provisional, // UNAuthorizationStatusProvisional
            4 => PermissionStatus::Granted, // UNAuthorizationStatusEphemeral
            _ => PermissionStatus::Denied,
        };
        let _ = tx.send(status);
    });
    let _: () = msg_send![center, getNotificationSettingsWithCompletionHandler: &*block];

    completion::wait(&rx).map_err(|_| "Failed to get notification settings".to_string())
}

unsafe fn check_bluetooth_authorization() -> Result<PermissionStatus, String> {
    let status: i64 = msg_send![class!(CBManager), authorization];
    Ok(match status {
        0 => PermissionStatus::Denied, // CBManagerAuthorizationNotDetermined
        1 => PermissionStatus::Restricted, // CBManagerAuthorizationRestricted
        2 => PermissionStatus::PermanentlyDenied, // CBManagerAuthorizationDenied
        3 => PermissionStatus::Granted, // CBManagerAuthorizationAllowedAlways
        _ => PermissionStatus::Denied,
    })
}

unsafe fn check_speech_authorization() -> Result<PermissionStatus, String> {
    let status: i64 = msg_send![class!(SFSpeechRecognizer), authorizationStatus];
    Ok(match status {
        0 => PermissionStatus::Denied, // SFSpeechRecognizerAuthorizationStatusNotDetermined
        1 => PermissionStatus::PermanentlyDenied, // SFSpeechRecognizerAuthorizationStatusDenied
        2 => PermissionStatus::Restricted, // SFSpeechRecognizerAuthorizationStatusRestricted
        3 => PermissionStatus::Granted, // SFSpeechRecognizerAuthorizationStatusAuthorized
        _ => PermissionStatus::Denied,
    })
}

unsafe fn check_tracking_authorization() -> Result<PermissionStatus, String> {
    let status: u32 = msg_send![class!(ATTrackingManager), trackingAuthorizationStatus];
    Ok(match status {
        0 => PermissionStatus::Denied, // ATTrackingManagerAuthorizationStatusNotDetermined
        1 => PermissionStatus::Restricted, // ATTrackingManagerAuthorizationStatusRestricted
        2 => PermissionStatus::PermanentlyDenied, // ATTrackingManagerAuthorizationStatusDenied
        3 => PermissionStatus::Granted, // ATTrackingManagerAuthorizationStatusAuthorized
        _ => PermissionStatus::Denied,
    })
}

// ── Request permission ──────────────────────────────────────────────────────

pub fn request_permission(permission: Permission) -> Result<PermissionStatus, String> {
    static REQUEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _request = REQUEST
        .try_lock()
        .map_err(|_| "Another permission request is active")?;
    // First check current status
    let current = check_permission(permission)?;
    if (current.is_granted()
        && !(permission == Permission::Calendar && current == PermissionStatus::Limited))
        || current == PermissionStatus::PermanentlyDenied
        || current == PermissionStatus::Restricted
    {
        return Ok(current);
    }

    unsafe {
        let key = match permission {
            Permission::Camera => Some("NSCameraUsageDescription"),
            Permission::Microphone => Some("NSMicrophoneUsageDescription"),
            Permission::Photos | Permission::Videos => Some("NSPhotoLibraryUsageDescription"),
            Permission::MediaLibrary => Some("NSAppleMusicUsageDescription"),
            Permission::Contacts => Some("NSContactsUsageDescription"),
            Permission::Speech => Some("NSSpeechRecognitionUsageDescription"),
            Permission::AppTrackingTransparency => Some("NSUserTrackingUsageDescription"),
            _ => None,
        };
        if let Some(key) = key {
            require_usage_description(key)?;
        }
        match permission {
            Permission::Camera => request_av_authorization("vide"),
            Permission::Microphone => request_av_authorization("soun"),
            Permission::Photos => request_photos_authorization(),
            Permission::LocationWhenInUse => location::request(false),
            Permission::LocationAlways => location::request(true),
            Permission::Contacts => request_contacts_authorization(),
            Permission::Calendar => request_event_authorization(0),
            Permission::Reminders => request_event_authorization(1),
            Permission::Notification => request_notification_authorization(),
            Permission::Speech => request_speech_authorization(),
            Permission::AppTrackingTransparency => request_tracking_authorization(),
            Permission::Bluetooth => bluetooth::request(),
            Permission::Sensors => {
                Err("Raw accelerometer/gyroscope permission requests are not exposed by iOS".into())
            }
            Permission::MediaLibrary => {
                let (tx, rx) = mpsc::channel();
                let block = block2::RcBlock::new(move |status: i64| {
                    let _ = tx.send(super::ios_status::media_library(status));
                });
                let _: () = msg_send![class!(MPMediaLibrary), requestAuthorization: &*block];
                completion::wait(&rx)
            }
            // Android-only
            Permission::Storage
            | Permission::SystemAlertWindow
            | Permission::InstallPackages
            | Permission::AccessNotificationPolicy
            | Permission::Phone
            | Permission::Sms
            | Permission::Audio => Err("This permission is not applicable on iOS".into()),
            Permission::Videos => request_photos_authorization(),
        }
    }
}

unsafe fn require_usage_description(key: &str) -> Result<(), String> {
    let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
    let value: *mut AnyObject = msg_send![bundle, objectForInfoDictionaryKey: nsstring(key)];
    if value.is_null() {
        return Err(format!("Missing Info.plist key {key}"));
    }
    let is_string: Bool = msg_send![value, isKindOfClass: class!(NSString)];
    if !is_string.as_bool() {
        return Err(format!("Info.plist {key} must be a string"));
    }
    let length: usize = msg_send![value, length];
    if length == 0 {
        return Err(format!("Info.plist {key} is empty"));
    }
    Ok(())
}

unsafe fn request_av_authorization(media_type: &str) -> Result<PermissionStatus, String> {
    let (tx, rx) = mpsc::channel();
    let ns_media = nsstring(media_type);

    let block = block2::RcBlock::new(move |granted: Bool| {
        let status = if granted.as_bool() {
            PermissionStatus::Granted
        } else {
            PermissionStatus::PermanentlyDenied
        };
        let _ = tx.send(status);
    });

    let _: () = msg_send![class!(AVCaptureDevice),
        requestAccessForMediaType: ns_media,
        completionHandler: &*block
    ];

    completion::wait(&rx).map_err(|_| "AV authorization request failed".to_string())
}

unsafe fn request_photos_authorization() -> Result<PermissionStatus, String> {
    let (tx, rx) = mpsc::channel();

    let block = block2::RcBlock::new(move |status: i64| {
        let perm = match status {
            0 => PermissionStatus::Denied,
            1 => PermissionStatus::Restricted,
            2 => PermissionStatus::PermanentlyDenied,
            3 => PermissionStatus::Granted,
            4 => PermissionStatus::Limited,
            _ => PermissionStatus::Denied,
        };
        let _ = tx.send(perm);
    });

    let _: () = msg_send![class!(PHPhotoLibrary),
        requestAuthorizationForAccessLevel: 1i64, // PHAccessLevelReadWrite
        handler: &*block
    ];

    completion::wait(&rx).map_err(|_| "Photos authorization request failed".to_string())
}

unsafe fn request_contacts_authorization() -> Result<PermissionStatus, String> {
    let (tx, rx) = mpsc::channel();
    let store: *mut AnyObject = msg_send![class!(CNContactStore), alloc];
    let store: *mut AnyObject = msg_send![store, init];

    if store.is_null() {
        return Err("Failed to create CNContactStore".into());
    }

    let block = block2::RcBlock::new(move |granted: Bool, error: *mut AnyObject| {
        if !error.is_null() {
            let _ = tx.send(Err(completion::error_message(error)));
            return;
        }
        let status = if granted.as_bool() {
            PermissionStatus::Granted
        } else {
            PermissionStatus::PermanentlyDenied
        };
        let _ = tx.send(Ok(status));
    });

    let _: () = msg_send![store,
        requestAccessForEntityType: 0i64, // CNEntityTypeContacts
        completionHandler: &*block
    ];

    let result = completion::wait(&rx);
    let _: () = msg_send![store, release];
    result??;
    check_contacts_authorization()
}

unsafe fn request_event_authorization(entity_type: i64) -> Result<PermissionStatus, String> {
    let (tx, rx) = mpsc::channel();
    let store: *mut AnyObject = msg_send![class!(EKEventStore), alloc];
    let store: *mut AnyObject = msg_send![store, init];
    if store.is_null() {
        return Err("Failed to create EKEventStore".into());
    }
    let selector = if entity_type == 0 {
        sel!(requestFullAccessToEventsWithCompletion:)
    } else {
        sel!(requestFullAccessToRemindersWithCompletion:)
    };
    let modern: Bool = msg_send![store, respondsToSelector: selector];
    let key = match (entity_type, modern.as_bool()) {
        (0, true) => "NSCalendarsFullAccessUsageDescription",
        (0, false) => "NSCalendarsUsageDescription",
        (_, true) => "NSRemindersFullAccessUsageDescription",
        (_, false) => "NSRemindersUsageDescription",
    };
    if let Err(error) = require_usage_description(key) {
        let _: () = msg_send![store, release];
        return Err(error);
    }

    let block = block2::RcBlock::new(move |granted: Bool, error: *mut AnyObject| {
        if !error.is_null() {
            let _ = tx.send(Err(completion::error_message(error)));
            return;
        }
        let status = if granted.as_bool() {
            PermissionStatus::Granted
        } else {
            PermissionStatus::PermanentlyDenied
        };
        let _ = tx.send(Ok(status));
    });

    if modern.as_bool() {
        if entity_type == 0 {
            let _: () = msg_send![store, requestFullAccessToEventsWithCompletion: &*block];
        } else {
            let _: () = msg_send![store, requestFullAccessToRemindersWithCompletion: &*block];
        }
    } else {
        let _: () = msg_send![store, requestAccessToEntityType: entity_type, completion: &*block];
    }

    let result = completion::wait(&rx);
    let _: () = msg_send![store, release];
    result?
}

unsafe fn request_notification_authorization() -> Result<PermissionStatus, String> {
    let (tx, rx) = mpsc::channel();
    let center: *mut AnyObject =
        msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];

    // UNAuthorizationOptionAlert | UNAuthorizationOptionBadge | UNAuthorizationOptionSound
    let options: u64 = (1 << 0) | (1 << 1) | (1 << 2);

    let block = block2::RcBlock::new(move |granted: Bool, error: *mut AnyObject| {
        if !error.is_null() {
            let _ = tx.send(Err(completion::error_message(error)));
            return;
        }
        let status = if granted.as_bool() {
            PermissionStatus::Granted
        } else {
            PermissionStatus::PermanentlyDenied
        };
        let _ = tx.send(Ok(status));
    });

    let _: () = msg_send![center,
        requestAuthorizationWithOptions: options,
        completionHandler: &*block
    ];

    completion::wait(&rx)?
}

unsafe fn request_speech_authorization() -> Result<PermissionStatus, String> {
    let (tx, rx) = mpsc::channel();

    let block = block2::RcBlock::new(move |status: i64| {
        let perm = match status {
            0 => PermissionStatus::Denied,
            1 => PermissionStatus::Denied,
            2 => PermissionStatus::Restricted,
            3 => PermissionStatus::Granted,
            _ => PermissionStatus::Denied,
        };
        let _ = tx.send(perm);
    });

    let _: () = msg_send![class!(SFSpeechRecognizer),
        requestAuthorization: &*block
    ];

    completion::wait(&rx).map_err(|_| "Speech authorization request failed".to_string())
}

unsafe fn request_tracking_authorization() -> Result<PermissionStatus, String> {
    let main: Bool = msg_send![class!(NSThread), isMainThread];
    if !main.as_bool() {
        return Err("Request tracking authorization on the main thread".into());
    }
    let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
    let state: i64 = msg_send![app, applicationState];
    if state != 0 {
        return Err("Tracking authorization requires an active application".into());
    }
    let (tx, rx) = mpsc::channel();

    let block = block2::RcBlock::new(move |status: u32| {
        let perm = match status {
            0 => PermissionStatus::Denied,
            1 => PermissionStatus::Restricted,
            2 => PermissionStatus::PermanentlyDenied,
            3 => PermissionStatus::Granted,
            _ => PermissionStatus::Denied,
        };
        let _ = tx.send(perm);
    });

    let _: () = msg_send![class!(ATTrackingManager),
        requestTrackingAuthorizationWithCompletionHandler: &*block
    ];

    completion::wait(&rx).map_err(|_| "Tracking authorization request failed".to_string())
}

// ── Batch request ───────────────────────────────────────────────────────────

pub fn request_permissions(
    permissions: &[Permission],
) -> Result<Vec<(Permission, PermissionStatus)>, String> {
    let mut results = Vec::with_capacity(permissions.len());
    for &perm in permissions {
        let status = request_permission(perm)?;
        results.push((perm, status));
    }
    Ok(results)
}

// ── Service status ──────────────────────────────────────────────────────────

pub fn service_status(permission: Permission) -> Result<ServiceStatus, String> {
    unsafe {
        match permission {
            Permission::LocationWhenInUse | Permission::LocationAlways => {
                let enabled: Bool = msg_send![class!(CLLocationManager), locationServicesEnabled];
                Ok(if enabled.as_bool() {
                    ServiceStatus::Enabled
                } else {
                    ServiceStatus::Disabled
                })
            }
            Permission::Bluetooth => bluetooth::service(),
            _ => Ok(ServiceStatus::NotApplicable),
        }
    }
}

// ── Open app settings ───────────────────────────────────────────────────────

pub fn open_app_settings() -> Result<bool, String> {
    unsafe {
        let main: Bool = msg_send![class!(NSThread), isMainThread];
        if !main.as_bool() {
            return Err("Open iOS settings on the main thread".into());
        }
        let url_string = nsstring("app-settings:");
        let url: *mut AnyObject = msg_send![class!(NSURL), URLWithString: url_string];
        if url.is_null() {
            return Err("Failed to create settings URL".into());
        }

        let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        let can_open: Bool = msg_send![app, canOpenURL: url];
        if !can_open.as_bool() {
            return Ok(false);
        }

        let empty_dict: *mut AnyObject = msg_send![class!(NSDictionary), dictionary];
        let (tx, rx) = mpsc::channel();
        let block = block2::RcBlock::new(move |opened: Bool| {
            let _ = tx.send(opened.as_bool());
        });
        let _: () = msg_send![app,
            openURL: url,
            options: empty_dict,
            completionHandler: &*block
        ];

        completion::wait(&rx)
    }
}
