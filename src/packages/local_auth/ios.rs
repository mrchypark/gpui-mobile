use super::{AuthResult, BiometricType};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};

/// LAPolicy constants.
const LA_POLICY_DEVICE_OWNER_AUTHENTICATION_WITH_BIOMETRICS: i64 = 1;

/// LABiometryType constants.
const LA_BIOMETRY_TYPE_NONE: i64 = 0;
const LA_BIOMETRY_TYPE_TOUCH_ID: i64 = 1;
const LA_BIOMETRY_TYPE_FACE_ID: i64 = 2;

/// LAError constants.
const LA_ERROR_AUTHENTICATION_FAILED: i64 = -1;
const LA_ERROR_USER_CANCEL: i64 = -2;
const LA_ERROR_SYSTEM_CANCEL: i64 = -4;
const LA_ERROR_PASSCODE_NOT_SET: i64 = -5;
const LA_ERROR_BIOMETRY_NOT_AVAILABLE: i64 = -6;
const LA_ERROR_BIOMETRY_NOT_ENROLLED: i64 = -7;
const LA_ERROR_BIOMETRY_LOCKOUT: i64 = -8;

unsafe fn create_la_context() -> *mut AnyObject {
    let context: *mut AnyObject = msg_send![class!(LAContext), alloc];
    let context: *mut AnyObject = msg_send![context, init];
    context
}

pub fn is_device_supported() -> Result<bool, String> {
    unsafe {
        let context = create_la_context();
        if context.is_null() {
            return Err("Failed to create LAContext".into());
        }

        // canEvaluatePolicy:error: — pass nil for error pointer
        let can_evaluate: bool = msg_send![
            context,
            canEvaluatePolicy: LA_POLICY_DEVICE_OWNER_AUTHENTICATION_WITH_BIOMETRICS,
            error: std::ptr::null_mut::<*mut AnyObject>()
        ];

        // Even if canEvaluatePolicy returns false (e.g. not enrolled), the device
        // may still have hardware. Check biometryType.
        let biometry_type: i64 = msg_send![context, biometryType];
        let _: () = msg_send![context, release];

        Ok(can_evaluate || biometry_type != LA_BIOMETRY_TYPE_NONE)
    }
}

pub fn can_authenticate() -> Result<bool, String> {
    unsafe {
        let context = create_la_context();
        if context.is_null() {
            return Err("Failed to create LAContext".into());
        }

        let can_evaluate: bool = msg_send![
            context,
            canEvaluatePolicy: LA_POLICY_DEVICE_OWNER_AUTHENTICATION_WITH_BIOMETRICS,
            error: std::ptr::null_mut::<*mut AnyObject>()
        ];

        let _: () = msg_send![context, release];
        Ok(can_evaluate)
    }
}

pub fn get_available_biometrics() -> Result<Vec<BiometricType>, String> {
    unsafe {
        let context = create_la_context();
        if context.is_null() {
            return Err("Failed to create LAContext".into());
        }

        // Must call canEvaluatePolicy first to populate biometryType
        let _can: bool = msg_send![
            context,
            canEvaluatePolicy: LA_POLICY_DEVICE_OWNER_AUTHENTICATION_WITH_BIOMETRICS,
            error: std::ptr::null_mut::<*mut AnyObject>()
        ];

        let biometry_type: i64 = msg_send![context, biometryType];
        let _: () = msg_send![context, release];

        let mut types = Vec::new();
        match biometry_type {
            LA_BIOMETRY_TYPE_TOUCH_ID => types.push(BiometricType::Fingerprint),
            LA_BIOMETRY_TYPE_FACE_ID => types.push(BiometricType::Face),
            _ => {}
        }

        Ok(types)
    }
}

pub fn authenticate(reason: &str) -> Result<AuthResult, String> {
    objc2::rc::autoreleasepool(|_| unsafe {
        let main: bool = msg_send![class!(NSThread), isMainThread];
        if main {
            return Err("Call authenticate from a worker thread".into());
        }
        if reason.trim().is_empty() {
            return Err("Authentication reason must not be empty".into());
        }
        let context = create_la_context();
        if context.is_null() {
            return Err("Failed to create LAContext".into());
        }
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let can_evaluate: bool = msg_send![context,
            canEvaluatePolicy: LA_POLICY_DEVICE_OWNER_AUTHENTICATION_WITH_BIOMETRICS,
            error: &mut error as *mut *mut AnyObject];
        if !can_evaluate {
            let result = auth_result(false, error);
            let _: () = msg_send![context, release];
            return Ok(result);
        }
        let reason = crate::ios::util::nsstring(reason);
        let (sender, receiver) = std::sync::mpsc::channel();
        // Native owns its block copy. A late reply owns only its sender, never a
        // raw Rust allocation or a semaphore released by the waiting thread.
        let reply = block2::RcBlock::new(move |success: Bool, error: *mut AnyObject| {
            let _ = sender.send(auth_result(success.as_bool(), error));
        });
        let _: () = msg_send![context,
            evaluatePolicy: LA_POLICY_DEVICE_OWNER_AUTHENTICATION_WITH_BIOMETRICS,
            localizedReason: reason, reply: &*reply];
        let result = receiver
            .recv_timeout(std::time::Duration::from_secs(120))
            .map_err(|_| "Authentication timed out or callback disconnected".to_owned());
        let _: () = msg_send![context, invalidate];
        let _: () = msg_send![context, release];
        result
    })
}

unsafe fn auth_result(success: bool, error: *mut AnyObject) -> AuthResult {
    if success {
        return AuthResult::Success;
    }
    if error.is_null() {
        return AuthResult::Failed;
    }
    let code: i64 = msg_send![error, code];
    match code {
        LA_ERROR_AUTHENTICATION_FAILED => AuthResult::Failed,
        LA_ERROR_USER_CANCEL | LA_ERROR_SYSTEM_CANCEL | -9 => AuthResult::ErrorUserCancelled,
        LA_ERROR_PASSCODE_NOT_SET => AuthResult::ErrorPasscodeNotSet,
        LA_ERROR_BIOMETRY_NOT_AVAILABLE => AuthResult::ErrorNotAvailable,
        LA_ERROR_BIOMETRY_NOT_ENROLLED => AuthResult::ErrorNotEnrolled,
        LA_ERROR_BIOMETRY_LOCKOUT => AuthResult::ErrorLockout,
        _ => AuthResult::ErrorOther,
    }
}
