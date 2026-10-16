//! Permission-only retained central, independent of the optional BLE/Tokio facade.
use super::{completion, PermissionStatus, ServiceStatus};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use std::sync::{mpsc, Mutex, OnceLock};

extern "C" {
    static CBCentralManagerOptionShowPowerAlertKey: *mut AnyObject;
}

struct Bridge {
    central: usize,
    delegate: usize,
    pending: Option<mpsc::Sender<()>>,
}
static BRIDGE: Mutex<Bridge> = Mutex::new(Bridge {
    central: 0,
    delegate: 0,
    pending: None,
});
static QUERY: Mutex<()> = Mutex::new(());
unsafe extern "C" fn changed(_: *mut AnyObject, _: Sel, central: *mut AnyObject) {
    if let Ok(bridge) = BRIDGE.lock() {
        if bridge.central == 0 || bridge.central == central as usize {
            if let Some(sender) = &bridge.pending {
                let _ = sender.send(());
            }
        }
    }
}
fn delegate_class() -> &'static AnyClass {
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| unsafe {
        let mut builder =
            ClassBuilder::new(c"GpuiPermissionBluetoothDelegate", class!(NSObject)).unwrap();
        builder.add_method(
            sel!(centralManagerDidUpdateState:),
            changed as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
        );
        builder.register()
    })
}
struct Pending;
impl Drop for Pending {
    fn drop(&mut self) {
        if let Ok(mut bridge) = BRIDGE.lock() {
            bridge.pending = None;
        }
    }
}
unsafe fn central() -> Result<*mut AnyObject, String> {
    let existing = BRIDGE
        .lock()
        .map_err(|_| "Bluetooth permission state poisoned")?
        .central;
    if existing != 0 {
        return Ok(existing as *mut AnyObject);
    }
    super::require_usage_description("NSBluetoothAlwaysUsageDescription")?;
    let delegate: *mut AnyObject = msg_send![delegate_class(), new];
    if delegate.is_null() {
        return Err("Cannot allocate Bluetooth authorization delegate".into());
    }
    let no: *mut AnyObject = msg_send![class!(NSNumber), numberWithBool: Bool::NO];
    let options: *mut AnyObject = msg_send![class!(NSDictionary), dictionaryWithObject: no,
        forKey: CBCentralManagerOptionShowPowerAlertKey];
    let manager: *mut AnyObject = msg_send![class!(CBCentralManager), alloc];
    let manager: *mut AnyObject = msg_send![manager, initWithDelegate: delegate, queue: std::ptr::null_mut::<AnyObject>(), options: options];
    if manager.is_null() {
        let _: () = msg_send![delegate, release];
        return Err("Cannot allocate Bluetooth authorization manager".into());
    }
    let mut bridge = BRIDGE
        .lock()
        .map_err(|_| "Bluetooth permission state poisoned")?;
    // Application-lifetime owner: reuse this exact manager for subsequent service polls.
    bridge.central = manager as usize;
    bridge.delegate = delegate as usize;
    Ok(manager)
}
unsafe fn main_thread() -> Result<(), String> {
    let main: Bool = msg_send![class!(NSThread), isMainThread];
    if main.as_bool() {
        Ok(())
    } else {
        Err("Query/request iOS Bluetooth state on the main thread".into())
    }
}
pub(super) unsafe fn request() -> Result<PermissionStatus, String> {
    main_thread()?;
    let _query = QUERY
        .try_lock()
        .map_err(|_| "Another Bluetooth permission/service query is active")?;
    let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
    let state: i64 = msg_send![app, applicationState];
    if state != 0 {
        return Err("Request Bluetooth permission while the app is active".into());
    }
    let (sender, receiver) = mpsc::channel();
    BRIDGE
        .lock()
        .map_err(|_| "Bluetooth permission state poisoned")?
        .pending = Some(sender);
    let _pending = Pending;
    let central = central()?;
    loop {
        let authorization: i64 = msg_send![class!(CBManager), authorization];
        if authorization != 0 {
            return super::check_bluetooth_authorization();
        }
        let state: i64 = msg_send![central, state];
        if state == 2 {
            return Err("Bluetooth is unsupported on this device".into());
        }
        completion::wait(&receiver)?;
    }
}
pub(super) unsafe fn service() -> Result<ServiceStatus, String> {
    main_thread()?;
    // A status query never triggers an authorization prompt.
    if super::check_bluetooth_authorization()? != PermissionStatus::Granted {
        return Err("Bluetooth authorization is required before querying radio state".into());
    }
    let _query = QUERY
        .try_lock()
        .map_err(|_| "Another Bluetooth permission/service query is active")?;
    let (sender, receiver) = mpsc::channel();
    BRIDGE
        .lock()
        .map_err(|_| "Bluetooth permission state poisoned")?
        .pending = Some(sender);
    let _pending = Pending;
    let central = central()?;
    loop {
        let state: i64 = msg_send![central, state];
        match state {
            5 => return Ok(ServiceStatus::Enabled),
            4 => return Ok(ServiceStatus::Disabled),
            2 => return Ok(ServiceStatus::NotApplicable),
            3 => return Err("Bluetooth authorization was revoked".into()),
            _ => completion::wait(&receiver)?, // Unknown/resetting: await native state.
        }
    }
}
