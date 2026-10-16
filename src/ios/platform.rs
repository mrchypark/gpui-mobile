//! iOS Platform implementation.
//!
//! This implements the Platform trait for iOS using UIKit.
//! Key differences from macOS:
//! - Uses UIApplication instead of NSApplication
//! - No menu bar (iOS apps don't have traditional menus)
//! - No windowed mode (iOS apps are always fullscreen on their display)
//! - Touch-based input instead of mouse
//! - System keyboard handling differs significantly

use super::util::nsstring as ns_string;
#[path = "platform_clipboard.rs"]
mod platform_clipboard;
#[cfg(feature = "notifications")]
#[path = "../platform_notifications.rs"]
mod platform_notifications;
#[path = "platform_preview.rs"]
mod platform_preview;
use super::{IosDispatcher, IosDisplay, IosWindow};
use anyhow::anyhow;
use futures::channel::oneshot;
use gpui::{
    Action, AnyWindowHandle, BackgroundExecutor, ClipboardItem, CursorStyle, DummyKeyboardMapper,
    ForegroundExecutor, Keymap, Menu, MenuItem, PathPromptOptions, Platform, PlatformDisplay,
    PlatformKeyboardLayout, PlatformKeyboardMapper, PlatformTextSystem, PlatformWindow, Result,
    Task, ThermalState, WindowAppearance, WindowParams,
};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use parking_lot::Mutex;
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

pub struct IosPlatform(Mutex<IosPlatformState>);

thread_local! {
    static SLEEP_CALLBACK: std::cell::RefCell<Option<Box<dyn FnMut()>>> = Default::default();
    static WAKE_CALLBACK: std::cell::RefCell<Option<Box<dyn FnMut()>>> = Default::default();
    static IDLE_GUARDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PREVIOUS_IDLE_DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) struct IosPlatformState {
    background_executor: BackgroundExecutor,
    foreground_executor: ForegroundExecutor,
    text_system: Arc<dyn PlatformTextSystem>,
    finish_launching: Option<Box<dyn FnOnce()>>,
    observers: std::collections::HashMap<&'static str, objc2::rc::Retained<AnyObject>>,
}

impl Default for IosPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl IosPlatform {
    pub fn new() -> Self {
        let dispatcher = Arc::new(IosDispatcher);

        let text_system: Arc<dyn PlatformTextSystem> = Arc::new(super::IosTextSystem::new());

        Self(Mutex::new(IosPlatformState {
            background_executor: BackgroundExecutor::new(dispatcher.clone()),
            foreground_executor: ForegroundExecutor::new(dispatcher),
            text_system,
            finish_launching: None,
            observers: Default::default(),
        }))
    }

    fn observe(&self, name: &'static str, mut callback: Box<dyn FnMut()>) {
        let events = Rc::new(crate::mobile_dispatch::EventQueue::<()>::default());
        events.set_handler(Some(Box::new(move |_| callback())));
        unsafe {
            let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
            let queue: *mut AnyObject = msg_send![class!(NSOperationQueue), mainQueue];
            let block = block2::RcBlock::new(move |_: *mut AnyObject| events.dispatch([()]));
            let observer: *mut AnyObject = msg_send![center, addObserverForName: ns_string(name),
                object: std::ptr::null::<AnyObject>(), queue: queue, usingBlock: &*block];
            if let Some(observer) = objc2::rc::Retained::retain(observer) {
                let previous = self.0.lock().observers.insert(name, observer);
                if let Some(previous) = previous {
                    let _: () = msg_send![center, removeObserver: &*previous];
                }
            }
        }
    }
}

impl Drop for IosPlatform {
    fn drop(&mut self) {
        unsafe {
            let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
            for (_, observer) in self.0.get_mut().observers.drain() {
                let _: () = msg_send![center, removeObserver: &*observer];
            }
        }
    }
}

/// A simple iOS keyboard layout.
struct IosKeyboardLayout;

impl PlatformKeyboardLayout for IosKeyboardLayout {
    fn id(&self) -> &str {
        "ios-default"
    }

    fn name(&self) -> &str {
        "iOS Default"
    }
}

impl Platform for IosPlatform {
    fn on_system_sleep(&self, callback: Box<dyn FnMut()>) {
        log::debug!("UIKit has no system-power sleep notification; app pause is separate");
        SLEEP_CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
    }

    fn on_system_wake(&self, callback: Box<dyn FnMut()>) {
        log::debug!("UIKit has no system-power wake notification; app resume is separate");
        WAKE_CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
    }

    fn is_cursor_visible(&self) -> bool {
        // UIKit manages connected pointer visibility; this backend never hides it.
        true
    }

    fn prevent_idle_sleep(&self, _reason: &str) -> Task<Result<gpui::ActivityGuard>> {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
            IDLE_GUARDS.with(|count| {
                if count.get() == 0 {
                    let previous: objc2::runtime::Bool = msg_send![app, isIdleTimerDisabled];
                    PREVIOUS_IDLE_DISABLED.with(|value| value.set(previous.as_bool()));
                    let _: () = msg_send![app, setIdleTimerDisabled: objc2::runtime::Bool::YES];
                }
                count.set(count.get() + 1);
            });
        }
        Task::ready(Ok(gpui::ActivityGuard::new(move || {
            // The guard may be dropped off-thread; UIKit changes stay on its main queue.
            unsafe extern "C" fn release(_: *mut std::ffi::c_void) {
                IDLE_GUARDS.with(|count| {
                        count.set(count.get() - 1);
                        if count.get() == 0 {
                            let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
                            let previous = PREVIOUS_IDLE_DISABLED.with(|value| value.get());
                            let _: () = msg_send![app, setIdleTimerDisabled: objc2::runtime::Bool::new(previous)];
                        }
                    });
            }
            unsafe extern "C" {
                fn dispatch_async_f(
                    queue: *mut std::ffi::c_void,
                    context: *mut std::ffi::c_void,
                    work: Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
                );
            }
            unsafe {
                dispatch_async_f(
                    super::dispatcher::dispatch_get_main_queue(),
                    std::ptr::null_mut(),
                    Some(release),
                );
            }
        })))
    }

    fn background_executor(&self) -> BackgroundExecutor {
        self.0.lock().background_executor.clone()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.0.lock().foreground_executor.clone()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.0.lock().text_system.clone()
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        #[cfg(feature = "notifications")]
        {
            platform_notifications::start(self.foreground_executor());
            self.observe(
                "UIApplicationDidBecomeActiveNotification",
                Box::new(platform_notifications::poll),
            );
        }
        // Store the callback for later invocation via FFI.
        // The callback will be invoked when gpui_ios_did_finish_launching() is called
        // from the iOS app delegate's applicationDidFinishLaunchingWithOptions:.
        self.0.lock().finish_launching = Some(on_finish_launching);

        // On iOS, the app lifecycle is managed by UIApplicationMain which must be
        // called from main() before any Rust code runs. The Application::run() method
        // is called during app initialization, before UIApplicationMain starts its
        // event loop.
        //
        // The finish_launching callback is stored and will be invoked when the iOS
        // app delegate calls gpui_ios_did_finish_launching() via FFI.
        //
        // Unlike macOS where we call NSApplication.run() here, on iOS we don't need
        // to start the run loop - UIApplicationMain handles that.
        //
        // The callback is forwarded to the FFI layer so it can be invoked from Obj-C.
        if let Some(callback) = self.0.lock().finish_launching.take() {
            super::ffi::set_finish_launching_callback(callback);
        }

        log::info!("GPUI iOS: Platform::run() completed, waiting for app delegate callback");
    }

    fn quit(&self) {
        // iOS apps cannot programmatically quit - they can only be terminated by the user
        // or the system. We can suspend to background though.
        log::warn!("iOS apps cannot programmatically quit");
    }

    fn restart(&self, _binary_path: Option<PathBuf>, _arguments: Vec<std::ffi::OsString>) {
        // iOS apps cannot restart themselves
        log::warn!("iOS apps cannot restart themselves");
    }

    fn activate(&self, _ignoring_other_apps: bool) {
        // iOS handles app activation automatically
    }

    fn hide(&self) {
        // iOS apps cannot hide themselves
    }

    fn hide_other_apps(&self) {
        // Not applicable on iOS
    }

    fn unhide_other_apps(&self) {
        // Not applicable on iOS
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        IosDisplay::all()
            .map(|display| Rc::new(display) as Rc<dyn PlatformDisplay>)
            .collect()
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(Rc::new(IosDisplay::main()))
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        super::ffi::active_window_handle()
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> anyhow::Result<Box<dyn PlatformWindow>> {
        let window = Box::new(IosWindow::new(handle, options)?);
        // Register the window with FFI layer so Objective-C can access it for rendering
        window.register_with_ffi();
        Ok(window)
    }

    fn window_appearance(&self) -> WindowAppearance {
        unsafe {
            let style: i64 = {
                let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
                let key_window: *mut AnyObject = msg_send![app, keyWindow];
                if key_window.is_null() {
                    return WindowAppearance::Light;
                }
                let trait_collection: *mut AnyObject = msg_send![key_window, traitCollection];
                msg_send![trait_collection, userInterfaceStyle]
            };

            // UIUserInterfaceStyle: 0 = unspecified, 1 = light, 2 = dark
            match style {
                2 => WindowAppearance::Dark,
                _ => WindowAppearance::Light,
            }
        }
    }

    fn open_url(&self, url: &str) {
        unsafe {
            let url_string = ns_string(url);
            let url: *mut AnyObject = msg_send![class!(NSURL), URLWithString: url_string];
            if url.is_null() {
                log::warn!("Cannot open malformed URL");
                return;
            }
            let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
            let options: *mut AnyObject = msg_send![class!(NSDictionary), dictionary];
            let _: () = msg_send![app, openURL: url, options: options, completionHandler: std::ptr::null::<AnyObject>()];
        }
    }

    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        crate::mobile_events::set_open_urls_handler(callback);
    }

    fn on_app_lifecycle(&self, callback: Box<dyn FnMut(gpui::AppLifecyclePhase)>) {
        crate::mobile_events::set_app_lifecycle_handler(callback);
    }

    fn on_memory_warning(&self, callback: Box<dyn FnMut()>) {
        crate::mobile_events::set_memory_warning_handler(callback);
    }

    fn register_url_scheme(&self, _url: &str) -> Task<Result<()>> {
        // URL schemes on iOS are registered in Info.plist, not programmatically
        Task::ready(Err(anyhow!("URL schemes must be declared in Info.plist")))
    }

    fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        let (tx, rx) = oneshot::channel();
        #[cfg(feature = "file_selector")]
        self.foreground_executor()
            .spawn(async move {
                let _ = tx.send(
                    crate::packages::file_selector::prompt_paths(options)
                        .map_err(anyhow::Error::msg),
                );
            })
            .detach();
        #[cfg(not(feature = "file_selector"))]
        {
            let _ = options;
            let _ = tx.send(Err(anyhow!("file_selector feature disabled")));
        }
        rx
    }

    fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        let (tx, rx) = oneshot::channel();
        #[cfg(feature = "file_selector")]
        let _ = tx.send(
            crate::packages::file_selector::prompt_new_path(directory, suggested_name)
                .map_err(anyhow::Error::msg),
        );
        #[cfg(not(feature = "file_selector"))]
        {
            let _ = (directory, suggested_name);
            let _ = tx.send(Err(anyhow!("file_selector feature disabled")));
        }
        rx
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }

    #[cfg(feature = "notifications")]
    fn show_system_notification(&self, notification: gpui::SystemNotification) {
        self.foreground_executor()
            .spawn(async move {
                if let Err(error) =
                    crate::packages::notifications::show_system_notification(&notification)
                {
                    log::warn!("Notification submission failed: {error}");
                }
            })
            .detach();
    }
    #[cfg(feature = "notifications")]
    fn dismiss_system_notification(&self, tag: &str) {
        if let Err(error) = crate::packages::notifications::dismiss_system_notification(tag) {
            log::warn!("Notification dismissal failed: {error}");
        }
    }
    #[cfg(feature = "notifications")]
    fn on_system_notification_response(
        &self,
        callback: Box<dyn FnMut(gpui::SystemNotificationResponse)>,
    ) {
        platform_notifications::start(self.foreground_executor());
        platform_notifications::set_handler(callback, self.foreground_executor());
    }

    fn reveal_path(&self, _path: &Path) {
        log::warn!("iOS does not expose desktop file-manager reveal semantics");
    }

    fn open_with_system(&self, path: &Path) {
        let path = path.to_owned();
        self.foreground_executor()
            .spawn(async move {
                if let Err(error) = platform_preview::open(&path) {
                    log::warn!("File preview failed: {error}");
                }
            })
            .detach();
    }

    fn on_quit(&self, mut callback: Box<dyn FnMut() -> bool>) {
        // UIKit's termination notification cannot be vetoed and is not sent for every kill.
        self.observe(
            "UIApplicationWillTerminateNotification",
            Box::new(move || {
                let _ = callback();
            }),
        );
    }

    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.observe("UIApplicationWillEnterForegroundNotification", callback);
    }

    fn set_menus(&self, _menus: Vec<Menu>, _keymap: &Keymap) {
        // iOS doesn't have a menu bar
        // Could potentially integrate with UIMenuBuilder for context menus
    }

    fn set_dock_menu(&self, _menu: Vec<MenuItem>, _keymap: &Keymap) {
        // iOS doesn't have a dock menu
    }

    fn on_app_menu_action(&self, _callback: Box<dyn FnMut(&dyn Action)>) {
        // Not applicable on iOS
    }

    fn on_will_open_app_menu(&self, _callback: Box<dyn FnMut()>) {
        // Not applicable on iOS
    }

    fn on_validate_app_menu_command(&self, _callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        // Not applicable on iOS
    }

    fn app_path(&self) -> Result<PathBuf> {
        unsafe {
            let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
            let path: *mut AnyObject = msg_send![bundle, bundlePath];
            let utf8: *const i8 = msg_send![path, UTF8String];
            if utf8.is_null() {
                return Err(anyhow!("Failed to get bundle path"));
            }
            let path_str = std::ffi::CStr::from_ptr(utf8).to_str()?;
            Ok(PathBuf::from(path_str))
        }
    }

    fn path_for_auxiliary_executable(&self, _name: &str) -> Result<PathBuf> {
        Err(anyhow!(
            "Launching auxiliary executables is not supported on iOS"
        ))
    }

    fn hide_cursor_until_mouse_moves(&self) {
        // UIKit owns the touch/pointer cursor and exposes no equivalent API.
    }

    fn set_cursor_style(&self, _style: CursorStyle) {
        // iOS doesn't have visible cursors (except for Apple Pencil hover on iPad)
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        true // iOS always auto-hides scrollbars
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        if let Err(error) = platform_clipboard::write(&item) {
            log::warn!("Clipboard publication failed: {error}");
        }
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        platform_clipboard::read().unwrap_or_else(|error| {
            log::warn!("Clipboard read failed: {error}");
            None
        })
    }

    fn read_from_clipboard_async(
        &self,
    ) -> Task<std::result::Result<Option<ClipboardItem>, gpui::ClipboardReadError>> {
        self.foreground_executor().spawn(async {
            platform_clipboard::read_async().await.map_err(|error| {
                log::warn!("Clipboard read failed: {error}");
                error
                    .downcast_ref::<gpui::ClipboardReadError>()
                    .cloned()
                    .unwrap_or(gpui::ClipboardReadError::UnsupportedContent)
            })
        })
    }

    fn write_credentials(&self, url: &str, username: &str, password: &[u8]) -> Task<Result<()>> {
        let (url, username, password) = (url.to_owned(), username.to_owned(), password.to_vec());
        self.background_executor().spawn(async move {
            objc2::rc::autoreleasepool(|_| unsafe {
                let query = credential_query(&url);
                let attributes: *mut AnyObject = msg_send![class!(NSMutableDictionary), dictionary];
                let mut payload = credential_payload::encode(&username, &password)?;
                let data: *mut AnyObject = msg_send![class!(NSData), dataWithBytes: payload.as_ptr().cast::<std::ffi::c_void>(), length: payload.len()];
                payload.fill(0);
                let _: () = msg_send![attributes, setObject: data, forKey: kSecValueData];
                let status = SecItemUpdate(query, attributes);
                if status != -25300 { return keychain_status(status); }
                let _: () = msg_send![query, addEntriesFromDictionary: attributes];
                let _: () = msg_send![query, setObject: kSecAttrAccessibleWhenUnlockedThisDeviceOnly, forKey: kSecAttrAccessible];
                let status = SecItemAdd(query, std::ptr::null_mut());
                // Another process may have inserted this service after our update.
                keychain_status(if status == -25299 {
                    SecItemUpdate(credential_query(&url), attributes)
                } else { status })
            })
        })
    }

    fn read_credentials(&self, url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        let url = url.to_owned();
        self.background_executor().spawn(async move {
            objc2::rc::autoreleasepool(|_| unsafe {
                let query = credential_query(&url);
                let yes: *mut AnyObject =
                    msg_send![class!(NSNumber), numberWithBool: objc2::runtime::Bool::YES];
                let _: () = msg_send![query, setObject: yes, forKey: kSecReturnData];
                let _: () = msg_send![query, setObject: kSecMatchLimitOne, forKey: kSecMatchLimit];
                let mut result = std::ptr::null_mut();
                let status = SecItemCopyMatching(query, &mut result);
                if status == -25300 {
                    return Ok(None);
                }
                keychain_status(status)?;
                let result = objc2::rc::Retained::<AnyObject>::from_raw(result)
                    .ok_or_else(|| anyhow!("Keychain returned no item"))?;
                let data = &*result;
                let bytes: *const std::ffi::c_void = msg_send![data, bytes];
                let length: usize = msg_send![data, length];
                let mut payload = if length == 0 {
                    Vec::new()
                } else {
                    if bytes.is_null() {
                        return Err(anyhow!("Keychain data is missing bytes"));
                    }
                    std::slice::from_raw_parts(bytes.cast(), length).to_vec()
                };
                let decoded = credential_payload::decode(&payload);
                payload.fill(0);
                Ok(Some(decoded?))
            })
        })
    }

    fn delete_credentials(&self, url: &str) -> Task<Result<()>> {
        let url = url.to_owned();
        self.background_executor().spawn(async move {
            objc2::rc::autoreleasepool(|_| unsafe {
                let status = SecItemDelete(credential_query(&url));
                if status == -25300 {
                    Ok(())
                } else {
                    keychain_status(status)
                }
            })
        })
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.observe("UITextInputCurrentInputModeDidChangeNotification", callback);
    }

    fn thermal_state(&self) -> ThermalState {
        unsafe {
            let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
            let state: isize = msg_send![info, thermalState];
            match state {
                1 => ThermalState::Fair,
                2 => ThermalState::Serious,
                3 => ThermalState::Critical,
                _ => ThermalState::Nominal,
            }
        }
    }

    fn on_thermal_state_change(&self, callback: Box<dyn FnMut()>) {
        crate::mobile_events::set_thermal_change_handler(callback);
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(IosKeyboardLayout)
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }
}

#[path = "../credential_payload.rs"]
mod credential_payload;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecClass: *mut AnyObject;
    static kSecClassGenericPassword: *mut AnyObject;
    static kSecAttrService: *mut AnyObject;
    static kSecAttrAccount: *mut AnyObject;
    static kSecValueData: *mut AnyObject;
    static kSecReturnData: *mut AnyObject;
    #[cfg(test)]
    static kSecMatchLimitAll: *mut AnyObject;
    static kSecMatchLimit: *mut AnyObject;
    static kSecMatchLimitOne: *mut AnyObject;
    static kSecAttrAccessible: *mut AnyObject;
    static kSecAttrAccessibleWhenUnlockedThisDeviceOnly: *mut AnyObject;
    fn SecItemAdd(query: *mut AnyObject, result: *mut *mut AnyObject) -> i32;
    fn SecItemUpdate(query: *mut AnyObject, attributes: *mut AnyObject) -> i32;
    fn SecItemCopyMatching(query: *mut AnyObject, result: *mut *mut AnyObject) -> i32;
    fn SecItemDelete(query: *mut AnyObject) -> i32;
}

unsafe fn credential_query(service: &str) -> *mut AnyObject {
    unsafe {
        let query: *mut AnyObject = msg_send![class!(NSMutableDictionary), dictionary];
        let _: () = msg_send![query, setObject: kSecClassGenericPassword, forKey: kSecClass];
        let _: () = msg_send![query, setObject: ns_string(service), forKey: kSecAttrService];
        let _: () = msg_send![query, setObject: ns_string("dev.gpui.platform.credentials.v1"), forKey: kSecAttrAccount];
        query
    }
}

fn keychain_status(status: i32) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(anyhow!("Keychain operation failed (OSStatus {status})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_errors_are_not_success_or_absence() {
        assert!(keychain_status(0).is_ok());
        assert!(keychain_status(-25300).is_err());
        assert!(keychain_status(-25293).is_err());
    }

    #[test]
    #[ignore = "requires an entitled unlocked iOS Keychain host"]
    fn concurrent_first_writes_keep_one_record_and_replace_serially() {
        let platform = IosPlatform::new();
        let service = format!("gpui-race-{:?}", std::time::SystemTime::now());
        let result: Result<()> = futures::executor::block_on(async {
            for second in ["first", "different"] {
                for _ in 0..16 {
                    platform.delete_credentials(&service).await?;
                    let a = platform.write_credentials(&service, "first", b"one");
                    let b = platform.write_credentials(&service, second, b"two");
                    let (a, b) = futures::join!(a, b);
                    a?;
                    b?;
                    let stored = platform.read_credentials(&service).await?;
                    if stored != Some(("first".into(), b"one".to_vec()))
                        && stored != Some((second.into(), b"two".to_vec()))
                    {
                        return Err(anyhow!("Concurrent write produced a torn credential"));
                    }
                    objc2::rc::autoreleasepool(|_| unsafe {
                        let query = credential_query(&service);
                        // Count all accounts for this unique service, not just our fixed account.
                        let _: () = msg_send![query, removeObjectForKey: kSecAttrAccount];
                        let yes: *mut AnyObject =
                            msg_send![class!(NSNumber), numberWithBool: objc2::runtime::Bool::YES];
                        let _: () = msg_send![query, setObject: yes, forKey: kSecReturnData];
                        let _: () =
                            msg_send![query, setObject: kSecMatchLimitAll, forKey: kSecMatchLimit];
                        let mut result = std::ptr::null_mut();
                        keychain_status(SecItemCopyMatching(query, &mut result))?;
                        let result = objc2::rc::Retained::<AnyObject>::from_raw(result)
                            .ok_or_else(|| anyhow!("Missing Keychain result"))?;
                        let count: usize = msg_send![&*result, count];
                        if count != 1 {
                            return Err(anyhow!("Expected one credential, got {count}"));
                        }
                        Ok(())
                    })?;
                    platform
                        .write_credentials(&service, "last", b"last")
                        .await?;
                    if platform.read_credentials(&service).await?
                        != Some(("last".into(), b"last".to_vec()))
                    {
                        return Err(anyhow!("Serialized replacement failed"));
                    }
                }
            }
            Ok(())
        });
        let cleanup = futures::executor::block_on(platform.delete_credentials(&service));
        cleanup.unwrap();
        result.unwrap();
    }

    #[test]
    #[ignore = "requires an iOS host with Keychain entitlement and an unlocked device"]
    fn credentials_persist_and_replace_username() {
        let platform = IosPlatform::new();
        let service = format!("gpui-platform-test-{:?}", std::time::SystemTime::now());
        let result: Result<()> = futures::executor::block_on(async {
            platform
                .write_credentials(&service, "first", b"\0secret")
                .await?;
            let first = platform.read_credentials(&service).await?;
            if first != Some(("first".into(), b"\0secret".to_vec())) {
                return Err(anyhow!("credential round trip mismatch"));
            }
            platform
                .write_credentials(&service, "second\0한", b"")
                .await?;
            if platform.read_credentials(&service).await? != Some(("second\0한".into(), Vec::new()))
            {
                return Err(anyhow!("credential replacement mismatch"));
            }
            Ok(())
        });
        futures::executor::block_on(platform.delete_credentials(&service)).unwrap();
        result.unwrap();
        assert!(
            futures::executor::block_on(platform.read_credentials(&service))
                .unwrap()
                .is_none()
        );
        futures::executor::block_on(platform.delete_credentials(&service)).unwrap();
    }

    #[test]
    fn native_string_preserves_embedded_nul_and_unicode() {
        objc2::rc::autoreleasepool(|_| unsafe {
            let text = "a\0한😀";
            let string = ns_string(text);
            let length: usize = msg_send![string, length];
            assert_eq!(length, text.encode_utf16().count());
            let nul: u16 = msg_send![string, characterAtIndex: 1usize];
            assert_eq!(nul, 0);
        });
    }
}
