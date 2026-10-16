//! FFI (Foreign Function Interface) module for iOS.
//!
//! This module exposes C-compatible functions that can be called from
//! Objective-C code in the iOS app delegate to initialize and control
//! the GPUI application lifecycle.
//!
//! ## Typical call sequence from Obj-C
//!
//! ```text
//! gpui_ios_run_demo()          // sets up platform + invokes finish-launching
//! gpui_ios_get_window()        // retrieve the GPUI window pointer
//! gpui_ios_request_frame(ptr)  // called every CADisplayLink tick
//! ```

use gpui::{App, AppContext, Application, RequestFrameOptions, WindowOptions};
use std::ffi::c_void;
use std::sync::OnceLock;

/// Global storage for the GPUI application state.
/// This is set during initialization and used by FFI callbacks.
static IOS_APP_STATE: OnceLock<IosAppState> = OnceLock::new();

thread_local! {
    // UIKit owns the run loop, so the strong GPUI handle must outlive run_app().
    static APPLICATION: std::cell::RefCell<Option<gpui::ApplicationHandle>> = const { std::cell::RefCell::new(None) };
}

/// Holds the state needed for iOS FFI callbacks.
/// Note: On iOS, all UI code runs on the main thread, so we use a RefCell
/// instead of Mutex and don't require Send.
struct IosAppState {
    /// The callback to invoke when the app finishes launching.
    /// This is the closure passed to Application::run().
    /// Using std::cell::UnsafeCell since this is only accessed from the main thread.
    finish_launching: std::cell::UnsafeCell<Option<Box<dyn FnOnce()>>>,
}

// Safety: On iOS, all GPUI operations happen on the main thread.
// The FFI functions are only called from the iOS app delegate which runs on main thread.
// We implement both Send and Sync because OnceLock requires Send for its value type,
// and we need Sync for the static. The actual access is always single-threaded.
unsafe impl Send for IosAppState {}
unsafe impl Sync for IosAppState {}

thread_local! {
    static WINDOWS: crate::mobile_dispatch::Registry<super::window::IosWindowInner> = Default::default();
}

pub(crate) fn windows() -> Vec<std::rc::Rc<super::window::IosWindowInner>> {
    WINDOWS.with(|registry| registry.snapshot())
}

pub(crate) fn active_window_handle() -> Option<gpui::AnyWindowHandle> {
    windows()
        .into_iter()
        .find_map(|window| window.active_handle())
}

pub(crate) fn window_for_handle(id: usize) -> Option<std::rc::Rc<super::window::IosWindowInner>> {
    WINDOWS
        .with(|registry| registry.get(id))
        .filter(|window| !window.closed.get())
}

pub(crate) fn unregister_window(id: usize) {
    WINDOWS.with(|registry| registry.remove(id));
}

/// Initialize the GPUI iOS application.
///
/// This should be called from `application:didFinishLaunchingWithOptions:`
/// in the iOS app delegate, before any other GPUI functions.
///
/// Returns a pointer to the app state that should be passed to other FFI functions.
/// Returns null if initialization fails.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_initialize() -> *mut c_void {
    log::info!("GPUI iOS: Initializing");

    // Initialize the app state
    let state = IosAppState {
        finish_launching: std::cell::UnsafeCell::new(None),
    };

    if IOS_APP_STATE.set(state).is_err() {
        log::error!("GPUI iOS: Already initialized");
        return std::ptr::null_mut();
    }

    // Return a non-null pointer to indicate success
    // The actual state is stored in the static
    std::ptr::dangling_mut::<c_void>()
}

/// Register a window with the FFI layer.
///
/// This is called internally when a new IosWindow is created.
/// The window pointer can then be retrieved by Objective-C code.
///
/// # Safety
/// This must only be called from the main thread.
pub(crate) fn register_window(window: &std::rc::Rc<super::window::IosWindowInner>) -> usize {
    install_platform_observers();
    WINDOWS.with(|registry| registry.insert(window))
}

fn install_platform_observers() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| unsafe {
        use objc2::{class, msg_send, runtime::AnyObject};
        let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
        let queue: *mut AnyObject = msg_send![class!(NSOperationQueue), mainQueue];
        // Process-lifetime observers capture no window or platform object.
        for (name, callback) in [
            (
                c"UIApplicationDidReceiveMemoryWarningNotification",
                crate::mobile_events::dispatch_memory_warning as fn(),
            ),
            (
                c"NSProcessInfoThermalStateDidChangeNotification",
                crate::mobile_events::dispatch_thermal_change as fn(),
            ),
        ] {
            let name: *mut AnyObject =
                msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
            let block = block2::RcBlock::new(move |_notification: *mut AnyObject| callback());
            let _: *mut AnyObject = msg_send![center, addObserverForName: name,
                object: std::ptr::null::<AnyObject>(), queue: queue, usingBlock: &*block];
        }
    });
}

/// Get the most recently created window pointer.
///
/// Returns the pointer to the IosWindow that was most recently registered,
/// or null if no windows have been created.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_get_window() -> *mut c_void {
    windows().last().map_or(std::ptr::null_mut(), |window| {
        window.native_handle() as *mut c_void
    })
}

/// Store the finish launching callback.
///
/// This is called internally by IosPlatform::run() to store the callback
/// that will be invoked when the app finishes launching.
///
/// # Safety
/// This must only be called from the main thread.
pub(crate) fn set_finish_launching_callback(callback: Box<dyn FnOnce()>) {
    if let Some(state) = IOS_APP_STATE.get() {
        // Safety: Only called from main thread
        unsafe {
            *state.finish_launching.get() = Some(callback);
        }
    }
}

/// Called when the iOS app has finished launching.
///
/// This should be called from `application:didFinishLaunchingWithOptions:`
/// in the iOS app delegate, after `gpui_ios_initialize()` returns.
///
/// This invokes the callback passed to Application::run().
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_did_finish_launching(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Did finish launching");

    if let Some(state) = IOS_APP_STATE.get() {
        // Safety: Only called from main thread
        let callback = unsafe { (*state.finish_launching.get()).take() };
        if let Some(callback) = callback {
            log::info!("GPUI iOS: Invoking finish launching callback");
            callback();
        } else {
            log::warn!("GPUI iOS: No finish launching callback registered");
        }
    } else {
        log::error!("GPUI iOS: Not initialized");
    }
}

/// Called when the iOS app will enter the foreground.
///
/// This should be called from `applicationWillEnterForeground:` in the app delegate.
/// This notifies all GPUI windows that the app is becoming active.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_will_enter_foreground(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Will enter foreground");
    crate::mobile_events::dispatch_app_lifecycle(gpui::AppLifecyclePhase::Foreground);

    // Notify all windows that they're becoming active
    for window in windows() {
        if !window.closed.get() {
            window.notify_active_status_change(true);
        }
    }
}

/// Called when the iOS app did become active.
///
/// This should be called from `applicationDidBecomeActive:` in the app delegate.
/// This indicates the app is now in the foreground and receiving events.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_did_become_active(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Did become active");
    crate::mobile_events::dispatch_app_lifecycle(gpui::AppLifecyclePhase::Active);

    // App is now fully active - windows should be notified
    for window in windows() {
        if !window.closed.get() {
            window.notify_active_status_change(true);
        }
    }
}

/// Called when the iOS app will resign active.
///
/// This should be called from `applicationWillResignActive:` in the app delegate.
/// This indicates the app is about to become inactive (e.g., incoming call, switching apps).
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_will_resign_active(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Will resign active");
    crate::mobile_events::dispatch_app_lifecycle(gpui::AppLifecyclePhase::Inactive);

    // App is about to become inactive
    for window in windows() {
        if !window.closed.get() {
            window.notify_active_status_change(false);
        }
    }
}

/// Called when the iOS app did enter the background.
///
/// This should be called from `applicationDidEnterBackground:` in the app delegate.
/// At this point, the app should have already saved any user data and released
/// shared resources. The app will be suspended shortly after this returns.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_did_enter_background(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Did enter background");
    crate::mobile_events::dispatch_app_lifecycle(gpui::AppLifecyclePhase::Background);

    // Notify windows they're no longer visible
    for window in windows() {
        if !window.closed.get() {
            window.notify_active_status_change(false);
        }
    }
}

/// Called when the iOS app will terminate.
///
/// This should be called from `applicationWillTerminate:` in the app delegate.
/// This is a good place to save any unsaved data.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_will_terminate(_app_ptr: *mut c_void) {
    log::info!("GPUI iOS: Will terminate");

    APPLICATION.with(|app| {
        app.borrow_mut().take();
    });
}

/// Called when a touch event occurs.
///
/// This bridges UIKit touch events to GPUI's input system.
/// Parameters:
/// - `window_ptr`: Pointer to the IosWindow
/// - `touch_ptr`: Pointer to the UITouch object
/// - `event_ptr`: Pointer to the UIEvent object
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_handle_touch(
    window_ptr: *mut c_void,
    touch_ptr: *mut c_void,
    event_ptr: *mut c_void,
) {
    if window_ptr.is_null() || touch_ptr.is_null() {
        return;
    }

    // Cast to IosWindow and forward the touch event
    let Some(window) = window_for_handle(window_ptr as usize) else {
        return;
    };
    window.handle_touch(
        touch_ptr as *mut objc2::runtime::AnyObject,
        event_ptr as *mut objc2::runtime::AnyObject,
    );
}

/// Request a frame to be rendered.
///
/// This should be called from CADisplayLink callback to trigger GPUI rendering.
/// The window_ptr should be the value returned by gpui_ios_get_window().
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_request_frame(window_ptr: *mut c_void) {
    if window_ptr.is_null() {
        return;
    }

    // Safety: window_ptr must be a valid pointer to an IosWindow
    let Some(window) = window_for_handle(window_ptr as usize) else {
        return;
    };

    // ── Momentum scrolling ───────────────────────────────────────────────
    if !window.can_render() {
        return;
    }
    // Pump the momentum scroller BEFORE the render callback so that any
    // synthetic ScrollWheel events are processed during this frame's
    // layout/paint cycle.  This produces the smooth, decelerating inertia
    // scroll that users expect on iOS after a fling gesture.
    window.pump_momentum();

    // Check if text input arrived since last frame — if so, force a render
    // so drain_pending_text() runs and the UI updates.
    let text_dirty = crate::TEXT_INPUT_DIRTY.swap(false, std::sync::atomic::Ordering::AcqRel);

    // Take the callback, invoke it, then restore it
    // We must complete the borrow before invoking the callback,
    // as the callback might try to borrow the same RefCell
    window.request_frame_callback.call(
        || !window.closed.get(),
        |cb| {
            cb(RequestFrameOptions {
                force_render: text_dirty,
                ..Default::default()
            });
        },
    );
}

/// Show the software keyboard.
///
/// Call this when a text input field gains focus.
/// The window_ptr should be the value returned by gpui_ios_get_window().
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_show_keyboard(window_ptr: *mut c_void) {
    if window_ptr.is_null() {
        return;
    }

    log::info!("GPUI iOS: Show keyboard requested");

    let Some(window) = window_for_handle(window_ptr as usize) else {
        return;
    };
    window.show_keyboard_with_type(crate::KeyboardType::Default);
}

/// Hide the software keyboard.
///
/// Call this when a text input field loses focus.
/// The window_ptr should be the value returned by gpui_ios_get_window().
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_hide_keyboard(window_ptr: *mut c_void) {
    if window_ptr.is_null() {
        return;
    }

    log::info!("GPUI iOS: Hide keyboard requested");

    let Some(window) = window_for_handle(window_ptr as usize) else {
        return;
    };
    window.hide_keyboard();
}

/// Handle text input from the software keyboard.
///
/// This is called when the user types on the keyboard.
/// Parameters:
/// - `window_ptr`: Pointer to the IosWindow
/// - `text_ptr`: Pointer to NSString with the entered text
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_handle_text_input(window_ptr: *mut c_void, text_ptr: *mut c_void) {
    if window_ptr.is_null() || text_ptr.is_null() {
        return;
    }

    log::info!("GPUI iOS: Handle text input");

    let Some(window) = window_for_handle(window_ptr as usize) else {
        return;
    };
    window.handle_text_input(text_ptr as *mut objc2::runtime::AnyObject);
}

/// Handle a key event from an external keyboard.
///
/// Parameters:
/// - `window_ptr`: Pointer to the IosWindow
/// - `key_code`: The key code from UIKeyboardHIDUsage
/// - `modifiers`: Modifier flags from UIKeyModifierFlags
/// - `is_key_down`: true for key down, false for key up
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_handle_key_event(
    window_ptr: *mut c_void,
    key_code: u32,
    modifiers: u32,
    is_key_down: bool,
) {
    if window_ptr.is_null() {
        return;
    }

    log::info!(
        "GPUI iOS: Handle key event - code: {}, modifiers: {}, down: {}",
        key_code,
        modifiers,
        is_key_down
    );

    let Some(window) = window_for_handle(window_ptr as usize) else {
        return;
    };
    window.handle_key_event(key_code, modifiers, is_key_down);
}

/// Called from ObjC when the app receives a URL to open.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_record_initial_url(url_ptr: *mut c_void) {
    #[cfg(feature = "deeplink")]
    {
        let url = if url_ptr.is_null() { None } else { unsafe {
            use objc2::msg_send;
            let string = url_ptr as *mut objc2::runtime::AnyObject;
            let bytes: *const u8 = msg_send![string, UTF8String];
            let length: usize = msg_send![string, lengthOfBytesUsingEncoding: 4usize];
            if bytes.is_null() { return; }
            match std::str::from_utf8(std::slice::from_raw_parts(bytes, length)) {
                Ok(url) => Some(url.to_owned()),
                Err(_) => return,
            }
        }};
        crate::packages::deeplink::ios::record_initial_link(url);
    }
    #[cfg(not(feature = "deeplink"))]
    let _ = url_ptr;
}

/// Called from ObjC when the app receives a URL to open.
///
/// Parameters:
/// - url_ptr: Pointer to an NSString containing the URL
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_handle_open_url(url_ptr: *mut c_void) {
    if url_ptr.is_null() {
        return;
    }

    let url_string = unsafe {
        use objc2::msg_send;
        let ns_str = url_ptr as *mut objc2::runtime::AnyObject;
        let cstr: *const std::ffi::c_char = msg_send![ns_str, UTF8String];
        if cstr.is_null() {
            return;
        }
        std::ffi::CStr::from_ptr(cstr)
            .to_string_lossy()
            .into_owned()
    };

    log::info!("GPUI iOS: Received deep link: {}", url_string);
    crate::mobile_events::dispatch_open_urls(vec![url_string.clone()]);

    #[cfg(feature = "deeplink")]
    {
        crate::packages::deeplink::ios::handle_open_url(url_string);
    }
}

// ── App callback storage ─────────────────────────────────────────────────────

/// Wrapper around an `UnsafeCell<Option<Box<dyn FnOnce(&mut App)>>>`.
///
/// # Safety
/// On iOS all UI work happens on the main thread.  The FFI entry points
/// (`set_app_callback`, `run_app`, `gpui_ios_run_demo`) are only ever
/// called from the main thread, so interior-mutable access is safe.
#[allow(clippy::type_complexity)]
struct AppCallbackCell(std::cell::UnsafeCell<Option<Box<dyn FnOnce(&mut App)>>>);

// Safety: only accessed from the iOS main thread.
unsafe impl Send for AppCallbackCell {}
unsafe impl Sync for AppCallbackCell {}

static APP_CALLBACK: OnceLock<AppCallbackCell> = OnceLock::new();

/// Register a callback that will be invoked inside `Application::run`.
///
/// This must be called **before** [`run_app`] so that the run-loop
/// has something to do (open a window, create views, etc.).
///
/// # Safety
/// Must be called from the main thread only.
pub fn set_app_callback(cb: Box<dyn FnOnce(&mut App)>) {
    let cell = APP_CALLBACK.get_or_init(|| AppCallbackCell(std::cell::UnsafeCell::new(None)));
    unsafe {
        *cell.0.get() = Some(cb);
    }
}

#[allow(clippy::type_complexity)]
fn take_app_callback() -> Option<Box<dyn FnOnce(&mut App)>> {
    APP_CALLBACK
        .get()
        .and_then(|cell| unsafe { (*cell.0.get()).take() })
}

/// C entry point called from `main.m`'s app delegate.
///
/// Consumer crates should call [`set_app_callback`] **before** this function
/// to register their root view.  If no callback is registered an empty
/// window is opened as a fallback.
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_run_demo() {
    run_app();
}

/// Run the GPUI iOS application.
///
/// This initialises the platform, creates the `Application`, and enters the
/// GPUI run loop.  The actual UI is determined by a callback previously
/// registered via [`set_app_callback`].  If no callback was registered a
/// default empty window is opened so the app doesn't crash.
pub fn run_app() {
    run_app_with_assets(());
}

/// Start the UIKit-hosted application with consumer-provided assets.
///
/// Call on the main thread after [`set_app_callback`]. The embedded application
/// handle is retained for subsequent UIKit lifecycle and display-link callbacks.
pub fn run_app_with_assets(assets: impl gpui::AssetSource) {
    log::info!("GPUI iOS: Starting application");

    // Initialise the FFI layer if not already done.
    if IOS_APP_STATE.get().is_none() {
        let state = IosAppState {
            finish_launching: std::cell::UnsafeCell::new(None),
        };
        let _ = IOS_APP_STATE.set(state);
    }

    let platform = super::current_platform(false);
    let application = Application::with_platform(platform).with_assets(assets);
    let application = application.run_embedded(|cx: &mut App| {
        if let Some(cb) = take_app_callback() {
            log::info!("GPUI iOS: Invoking user-provided app callback");
            cb(cx);
        } else {
            log::warn!("GPUI iOS: No app callback registered — opening default empty window");
            cx.open_window(
                WindowOptions {
                    window_bounds: None,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| gpui::Empty),
            )
            .expect("Failed to open default window");
            cx.activate(true);
        }
    });

    APPLICATION.with(|app| *app.borrow_mut() = Some(application));

    // On iOS, run_embedded() stores the callback and returns immediately.
    // The finish-launching callback is forwarded to set_finish_launching_callback
    // and invoked here synchronously (in a real app the app delegate does this).
    if let Some(state) = IOS_APP_STATE.get() {
        let callback = unsafe { (*state.finish_launching.get()).take() };
        if let Some(callback) = callback {
            log::info!("GPUI iOS: Invoking Application::run callback");
            callback();
        }
    }
}
