//! Permission-only CoreLocation owner. Does not start location updates or touch GPS owners.
use super::{completion, require_usage_description, PermissionStatus};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use std::ffi::c_void;
use std::sync::{mpsc, OnceLock};

const ALWAYS_REQUESTED: &str = "gpui.permission.always-location-requested";
#[link(name = "UIKit", kind = "framework")]
extern "C" {
    static UIApplicationWillResignActiveNotification: *mut AnyObject;
    static UIApplicationDidBecomeActiveNotification: *mut AnyObject;
    static UIApplicationDidEnterBackgroundNotification: *mut AnyObject;
}
struct State {
    manager: *mut AnyObject,
    always: bool,
    requested: bool,
    resigned: bool,
    sender: Option<mpsc::Sender<Result<PermissionStatus, String>>>,
}
unsafe fn state_ptr(this: *mut AnyObject) -> *mut State {
    *(&*this)
        .class()
        .instance_variable(c"gpui_authorization")
        .unwrap()
        .load::<*mut c_void>(&*this) as *mut State
}
unsafe fn finish(this: *mut AnyObject, result: Result<PermissionStatus, String>) {
    if let Some(state) = state_ptr(this).as_mut() {
        if let Some(sender) = state.sender.take() {
            let _ = sender.send(result);
        }
    }
}
unsafe extern "C" fn changed(this: *mut AnyObject, _: Sel, manager: *mut AnyObject) {
    let status: i32 = msg_send![manager, authorizationStatus];
    let Some(state) = state_ptr(this).as_mut() else {
        return;
    };
    if state.manager != manager || state.sender.is_none() {
        return;
    }
    let mapped = super::super::ios_status::location(status, state.always);
    if state.requested {
        if status != 0 {
            finish(this, Ok(mapped));
        }
        return;
    }
    // The first manager callback is only its initial status; initiate the request here,
    // so it cannot be mistaken for the user's authorization decision.
    if mapped != PermissionStatus::Denied {
        finish(this, Ok(mapped));
        return;
    }
    state.requested = true;
    let always = state.always;
    if always {
        let defaults: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let _: () =
            msg_send![defaults, setBool: Bool::YES, forKey: super::nsstring(ALWAYS_REQUESTED)];
        let _: () = msg_send![manager, requestAlwaysAuthorization];
    } else {
        let _: () = msg_send![manager, requestWhenInUseAuthorization];
    }
}
unsafe extern "C" fn resigned(this: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    if let Some(state) = state_ptr(this).as_mut() {
        if state.requested {
            state.resigned = true;
        }
    }
}
unsafe extern "C" fn active(this: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    let Some(state) = state_ptr(this).as_ref() else {
        return;
    };
    if !state.requested || !state.resigned || state.sender.is_none() {
        return;
    }
    // An unchanged Always upgrade denial has no authorization-change notification.
    // Native activation after the prompt closes is the terminal signal in that case.
    let status: i32 = msg_send![state.manager, authorizationStatus];
    let result = if status == 0 {
        Err("Location prompt closed without determining authorization".into())
    } else {
        Ok(super::super::ios_status::location(status, state.always))
    };
    finish(this, result);
}
unsafe extern "C" fn backgrounded(this: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    finish(
        this,
        Err("Location authorization interrupted by app backgrounding".into()),
    );
}
fn delegate_class() -> &'static AnyClass {
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| unsafe {
        let mut builder =
            ClassBuilder::new(c"GpuiPermissionLocationDelegate", class!(NSObject)).unwrap();
        builder.add_ivar::<*mut c_void>(c"gpui_authorization");
        builder.add_method(
            sel!(locationManagerDidChangeAuthorization:),
            changed as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
        );
        builder.add_method(
            sel!(gpuiResigned:),
            resigned as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
        );
        builder.add_method(
            sel!(gpuiActive:),
            active as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
        );
        builder.add_method(
            sel!(gpuiBackgrounded:),
            backgrounded as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
        );
        builder.register()
    })
}
struct Owner {
    manager: *mut AnyObject,
    delegate: *mut AnyObject,
}
impl Drop for Owner {
    fn drop(&mut self) {
        unsafe {
            let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![center, removeObserver: self.delegate];
            let _: () = msg_send![self.manager, setDelegate: std::ptr::null_mut::<AnyObject>()];
            let ptr = state_ptr(self.delegate);
            *(&*self.delegate)
                .class()
                .instance_variable(c"gpui_authorization")
                .unwrap()
                .load_ptr::<*mut c_void>(&*self.delegate) = std::ptr::null_mut();
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
            let _: () = msg_send![self.manager, release];
            let _: () = msg_send![self.delegate, release];
        }
    }
}
pub(super) unsafe fn check(always: bool) -> Result<PermissionStatus, String> {
    // The class accessor is retained by CoreLocation for compatibility; allocates no manager.
    let status: i32 = msg_send![class!(CLLocationManager), authorizationStatus];
    Ok(super::super::ios_status::location(status, always))
}
pub(super) unsafe fn request(always: bool) -> Result<PermissionStatus, String> {
    let main: Bool = msg_send![class!(NSThread), isMainThread];
    if !main.as_bool() {
        return Err("Request iOS location permission on the main thread".into());
    }
    let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
    let active: i64 = msg_send![app, applicationState];
    if active != 0 {
        return Err("Request location permission while the app is active".into());
    }
    let enabled: Bool = msg_send![class!(CLLocationManager), locationServicesEnabled];
    if !enabled.as_bool() {
        return Err("Location services are disabled".into());
    }
    require_usage_description("NSLocationWhenInUseUsageDescription")?;
    if always {
        require_usage_description("NSLocationAlwaysAndWhenInUseUsageDescription")?;
        let defaults: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let requested: Bool = msg_send![defaults, boolForKey: super::nsstring(ALWAYS_REQUESTED)];
        if requested.as_bool() {
            return Err(
                "Always location was already requested; change its scope in Settings".into(),
            );
        }
    }
    let manager: *mut AnyObject = msg_send![class!(CLLocationManager), new];
    if manager.is_null() {
        return Err("Cannot create location authorization manager".into());
    }
    let delegate: *mut AnyObject = msg_send![delegate_class(), new];
    if delegate.is_null() {
        let _: () = msg_send![manager, release];
        return Err("Cannot create location authorization delegate".into());
    }
    let (sender, receiver) = mpsc::channel();
    let state = Box::new(State {
        manager,
        always,
        requested: false,
        resigned: false,
        sender: Some(sender),
    });
    *(&*delegate)
        .class()
        .instance_variable(c"gpui_authorization")
        .unwrap()
        .load_ptr::<*mut c_void>(&*delegate) = Box::into_raw(state).cast();
    let _owner = Owner { manager, delegate };
    let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
    let _: () = msg_send![center, addObserver: delegate, selector: sel!(gpuiResigned:), name: UIApplicationWillResignActiveNotification, object: app];
    let _: () = msg_send![center, addObserver: delegate, selector: sel!(gpuiActive:), name: UIApplicationDidBecomeActiveNotification, object: app];
    let _: () = msg_send![center, addObserver: delegate, selector: sel!(gpuiBackgrounded:), name: UIApplicationDidEnterBackgroundNotification, object: app];
    let _: () = msg_send![manager, setDelegate: delegate];
    completion::wait(&receiver)?
}
