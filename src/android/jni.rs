//! Android entry point and event loop using the `android-activity` crate.
//!
//! This module replaces the previous hand-rolled `ANativeActivity_onCreate`,
//! `JNI_OnLoad`, and lifecycle callback implementations with the higher-level
//! `android-activity` glue layer.
//!
//! ## Entry sequence
//!
//! ```text
//! android-activity loads the .so and calls android_main(app: AndroidApp)
//!   └── We store the AndroidApp globally
//!       └── Call the user-supplied gpui_android_main(app)
//! ```
//!
//! ## Threading model
//!
//! `android-activity` spawns a dedicated native thread and calls `android_main`
//! on it.  All GPUI draw / event callbacks run on this thread.  The
//! `AndroidApp` handle is `Send + Sync` and can be shared across threads.
//!
//! ## User entry point
//!
//! Applications must define:
//!
//! ```rust,no_run
//! #[no_mangle]
//! fn android_main(app: android_activity::AndroidApp) {
//!     // Initialise GPUI and run the application.
//! }
//! ```
//!
//! ## Event handling
//!
//! Lifecycle events (window creation/destruction, focus changes, etc.) are
//! delivered via `AndroidApp::poll_events()`.  Input events are obtained via
//! `AndroidApp::input_events_iter()`.

#![allow(unsafe_code)]
#![allow(non_snake_case)]

use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};

/// Whether the deferred init-window callback has already been invoked.
///
/// Reset to `false` on `TerminateWindow` so that when the surface is
/// recreated on resume the init callbacks run again.
static INIT_WINDOW_DONE: AtomicBool = AtomicBool::new(false);

thread_local! {
    static NATIVE_TOUCH_ROUTE: std::cell::RefCell<crate::mobile_dispatch::NativeTouchRoute> = Default::default();
    static PLATFORM_EVENTS: std::cell::RefCell<Vec<Option<gpui::AppLifecyclePhase>>> = Default::default();
}

fn queue_platform_event(phase: Option<gpui::AppLifecyclePhase>) {
    PLATFORM_EVENTS.with(|events| events.borrow_mut().push(phase));
}

static THERMAL_NEXT_GENERATION: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
static THERMAL_GENERATION: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static THERMAL_STOP_PENDING: AtomicBool = AtomicBool::new(false);
static THERMAL_EVENTS: std::sync::Mutex<Vec<(i64, i32)>> = std::sync::Mutex::new(Vec::new());

/// Invalidate the previous registration before scheduling Java-main registration.
pub(crate) fn begin_thermal_listener() -> i64 {
    let generation = THERMAL_NEXT_GENERATION.fetch_add(1, Ordering::AcqRel);
    THERMAL_GENERATION.store(generation, Ordering::Release);
    generation
}

pub(crate) fn end_thermal_listener(generation: i64) {
    let _ = THERMAL_GENERATION.compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire);
}

/// Java supplies only owned scalar values; no GPUI callback runs on this thread.
#[unsafe(no_mangle)]
pub extern "C" fn Java_dev_gpui_mobile_GpuiThermal_nativeStatusChanged(
    _env: *mut c_void,
    _class: *mut c_void,
    generation: i64,
    status: i32,
) {
    if generation <= 0
        || !(0..=6).contains(&status)
        || THERMAL_GENERATION.load(Ordering::Acquire) != generation
    {
        return;
    }
    THERMAL_EVENTS.lock().unwrap().push((generation, status));
    if let Some(app) = ANDROID_APP.get() {
        app.create_waker().wake();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_dev_gpui_mobile_GpuiThermal_nativeListenerStopped(
    _env: *mut c_void,
    _class: *mut c_void,
    generation: i64,
) {
    if generation > 0
        && THERMAL_GENERATION
            .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        THERMAL_STOP_PENDING.store(true, Ordering::Release);
        if let Some(app) = ANDROID_APP.get() {
            app.create_waker().wake();
        }
    }
}

fn drain_platform_events() {
    // Outside drawing/App borrows; resolve actual installed identity before draining IME.
    if let Some(window) = PLATFORM
        .get()
        .and_then(|platform| platform.primary_window())
    {
        window.refresh_ime_owner();
    }
    if THERMAL_STOP_PENDING.swap(false, Ordering::AcqRel)
        && THERMAL_GENERATION.load(Ordering::Acquire) == 0
    {
        if let Some(platform) = PLATFORM.get() {
            platform.on_thermal_host_destroyed();
        }
    }
    let thermal = std::mem::take(&mut *THERMAL_EVENTS.lock().unwrap());
    if let Some(platform) = PLATFORM.get() {
        for (generation, status) in thermal {
            if THERMAL_GENERATION.load(Ordering::Acquire) == generation {
                platform.deliver_thermal_status(generation, status);
            }
        }
    }
    let urls = std::mem::take(&mut *OPEN_URLS_PENDING.lock().unwrap());
    if !urls.is_empty() {
        if let Some(platform) = PLATFORM.get() {
            platform.deliver_open_urls(urls);
        }
    }
    let text = std::mem::take(&mut *IME_TEXT.lock().unwrap());
    if let Some(window) = PLATFORM
        .get()
        .and_then(|platform| platform.primary_window())
    {
        for (session, editor, input) in text {
            if let Some(lease) = crate::mobile_events::active_text_input_lease() {
                if !window
                    .gpui_handle()
                    .is_some_and(|handle| handle.window_id().as_u64() == lease.window())
                {
                    continue;
                }
            }
            log::info!("IME drain session={session} editor={editor} active={} current_session={} current_editor={} accepted={}",
                IME_ACTIVE.load(Ordering::Acquire), IME_GENERATION.load(Ordering::Acquire),
                IME_EDITOR.load(Ordering::Acquire), ime_delivery_current(session, editor));
            if ime_delivery_current(session, editor) {
                window.handle_ime_input(session, editor, input);
            }
        }
    }
    let events = PLATFORM_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
    for phase in events {
        if let Some(phase) = phase {
            crate::mobile_events::dispatch_app_lifecycle(phase);
        } else {
            crate::mobile_events::dispatch_memory_warning();
        }
    }
}

/// Whether the GPUI native library has completed initialization.
///
/// Set to `true` after the first frame is rendered. This can be queried
/// via JNI by a custom Activity to dismiss the splash screen, although
/// with NativeActivity the system splash handles this automatically.
pub static NATIVE_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Deferred lifecycle flags.
///
/// We must NOT call `win.set_active()` or `platform.did_enter_background()`
/// inside `handle_main_event` (which runs within `poll_events`).
/// The android-activity crate's Java-side callbacks block on a condvar
/// waiting for the native thread to finish processing the command.
/// If our handler tries to acquire the window `state` lock (a
/// `parking_lot::Mutex`), and a background thread is holding it (e.g.
/// during a render pass), we deadlock: native waits on the lock,
/// the lock holder waits for native rendering to complete, but native
/// is stuck.
///
/// Instead, the handlers set these flags and the main loop body
/// processes them AFTER `poll_events` returns.
static PAUSE_PENDING: AtomicBool = AtomicBool::new(false);
static RESUME_PENDING: AtomicBool = AtomicBool::new(false);
static FOCUS_PENDING: std::sync::Mutex<Vec<bool>> = std::sync::Mutex::new(Vec::new());
static TERM_WINDOW_PENDING: AtomicBool = AtomicBool::new(false);
static INIT_WINDOW_PENDING: AtomicBool = AtomicBool::new(false);
static WINDOW_RESIZED_PENDING: AtomicBool = AtomicBool::new(false);
static CONFIG_CHANGED_PENDING: AtomicBool = AtomicBool::new(false);
static WINDOW_INSETS_PENDING: parking_lot::Mutex<
    Option<(f32, crate::mobile_events::WindowInsets)>,
> = parking_lot::Mutex::new(None);

/// Java UI callbacks only publish data; GPUI dispatch runs on its native thread.
#[no_mangle]
pub unsafe extern "system" fn Java_dev_gpui_mobile_GpuiInsets_nativeInsets(
    _env: *mut c_void,
    _class: *mut c_void,
    density: f32,
    top: i32,
    bottom: i32,
    left: i32,
    right: i32,
    ime_top: i32,
    ime_bottom: i32,
    ime_left: i32,
    ime_right: i32,
    ime_visible: jni::sys::jboolean,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    if !density.is_finite()
        || density <= 0.0
        || [
            top, bottom, left, right, ime_top, ime_bottom, ime_left, ime_right, x, y, width, height,
        ]
        .iter()
        .any(|value| *value < 0)
    {
        return;
    }
    let logical = |value: i32| gpui::px(value as f32 / density);
    let insets = crate::mobile_events::WindowInsets {
        safe_area: gpui::Edges {
            top: logical(top),
            bottom: logical(bottom),
            left: logical(left),
            right: logical(right),
        },
        keyboard: (ime_visible && width > 0 && height > 0).then(|| gpui::Bounds {
            origin: gpui::point(logical(x), logical(y)),
            size: gpui::size(logical(width), logical(height)),
        }),
        keyboard_visible: Some(ime_visible),
        keyboard_insets: Some(gpui::Edges {
            top: logical(ime_top),
            bottom: logical(ime_bottom),
            left: logical(ime_left),
            right: logical(ime_right),
        }),
    };
    *WINDOW_INSETS_PENDING.lock() = Some((density, insets));
    if let Some(app) = ANDROID_APP.get() {
        app.create_waker().wake();
    }
}

use android_activity::{AndroidApp, MainEvent, PollEvent};

use super::platform::{AndroidPlatform, SharedPlatform};

use jni::objects::{JObject, JString, JValue};
use jni::JavaVM;

// ── JNI helpers (safe `jni` crate wrappers) ──────────────────────────────────

static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

/// Get or create the static `JavaVM` wrapper.
fn java_vm_safe() -> Result<&'static JavaVM, String> {
    if let Some(vm) = JAVA_VM.get() {
        return Ok(vm);
    }
    let ptr = java_vm();
    if ptr.is_null() {
        return Err("JavaVM not available".into());
    }
    Ok(JAVA_VM.get_or_init(|| unsafe { JavaVM::from_raw(ptr as *mut jni::sys::JavaVM) }))
}

/// Run a closure with an attached `jni::Env` for the current thread.
///
/// In jni 0.22 the `attach_current_thread` API is closure-based.
/// The thread is auto-detached when the closure returns (if it was
/// not already attached).
pub fn with_env<T>(f: impl FnOnce(&mut jni::Env) -> Result<T, String>) -> Result<T, String> {
    let vm = java_vm_safe()?;
    let mut result: Option<Result<T, String>> = None;
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), jni::errors::Error> {
        result = Some(f(env));
        Ok(())
    })
    .map_err(|e: jni::errors::Error| e.to_string())?;
    result.unwrap()
}

/// Convenience alias: kept so existing callers that import `obtain_env`
/// compile with minimal changes. Returns a result by running the given
/// closure inside `with_env`.
#[inline]
pub fn obtain_env<T>(f: impl FnOnce(&mut jni::Env) -> Result<T, String>) -> Result<T, String> {
    with_env(f)
}

/// Get the Activity as a [`JObject`].
///
/// `activity_as_ptr()` returns a JNI global reference from `android-activity`
/// that is valid for the lifetime of the app. We wrap it in a `JObject`.
///
/// Requires `&Env` because jni 0.22's `JObject::from_raw` binds the
/// local-reference-frame lifetime.
pub fn activity<'local>(env: &jni::Env<'local>) -> Result<JObject<'local>, String> {
    let ptr = activity_as_ptr();
    if ptr.is_null() {
        return Err("Activity not available".into());
    }
    Ok(unsafe { JObject::from_raw(env, ptr as jni::sys::jobject) })
}

/// Convert a Java String (`JObject` wrapping a `java.lang.String`) to a Rust `String`.
///
/// Returns an empty string on null or error.
pub fn get_string(env: &mut jni::Env<'_>, obj: &JObject<'_>) -> String {
    if obj.is_null() {
        return String::new();
    }
    let jstr = unsafe { JString::from_raw(env, obj.as_raw()) };
    jstr.to_string()
}

/// Extension trait for converting `jni::errors::Result<T>` to `Result<T, String>`.
pub(crate) trait JniExt<T> {
    fn e(self) -> Result<T, String>;
}

impl<T> JniExt<T> for jni::errors::Result<T> {
    fn e(self) -> Result<T, String> {
        self.map_err(|e| e.to_string())
    }
}

/// Find an application class by name using the Activity's classloader.
///
/// From native threads, `JNIEnv::FindClass` uses the system classloader
/// which doesn't know about application classes.  This helper uses the
/// Activity's classloader via `activity.getClass().getClassLoader().loadClass(name)`.
///
/// `class_name` uses Java dot notation (e.g. `"dev.gpui.mobile.GpuiHelper"`).
pub fn find_app_class<'local>(
    env: &mut jni::Env<'local>,
    class_name: &str,
) -> Result<jni::objects::JClass<'local>, String> {
    let act = activity(env)?;

    // activity.getClassLoader() — call on the Context instance directly.
    // Do NOT use activity.getClass().getClassLoader(): NativeActivity is a
    // framework class loaded by BootClassLoader, which cannot see app classes.
    let class_loader = env
        .call_method(
            &act,
            jni::jni_str!("getClassLoader"),
            jni::jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )
        .and_then(|v| v.l())
        .map_err(|e| {
            env.exception_clear();
            let msg = format!("getClassLoader failed: {e}");
            log::error!("find_app_class({class_name}): {msg}");
            msg
        })?;

    // classLoader.loadClass("dev.gpui.mobile.GpuiHelper")
    let jname = env.new_string(class_name).e()?;
    let loaded = env
        .call_method(
            &class_loader,
            jni::jni_str!("loadClass"),
            jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&jname)],
        )
        .and_then(|v| v.l())
        .map_err(|e| {
            // Print full Java stack trace to logcat, then clear.
            env.exception_describe();
            env.exception_clear();
            let msg = format!("loadClass({class_name}) failed: {e}");
            log::error!("{msg}");
            msg
        })?;

    log::debug!("find_app_class: loaded {class_name}");
    Ok(unsafe { jni::objects::JClass::from_raw(env, loaded.as_raw()) })
}

// ── global state ─────────────────────────────────────────────────────────────

/// The `AndroidApp` handle from `android-activity`.
///
/// Set once in `android_main`; read-only thereafter.
static ANDROID_APP: OnceLock<AndroidApp> = OnceLock::new();

/// Process-global `AndroidPlatform` instance.
///
/// Initialised once during `android_main`; read-only thereafter.
static PLATFORM: OnceLock<Arc<AndroidPlatform>> = OnceLock::new();

/// Get the unicode character produced by an Android key event via JNI.
///
/// This creates a `android.view.KeyEvent` Java object and calls
/// `getUnicodeChar(metaState)` on it.  Returns 0 on failure.
pub fn unicode_char_for_key_event(key_code: i32, action: i32, meta_state: i32) -> u32 {
    with_env(|env| {
        let key_event = match env.new_object(
            jni::jni_str!("android/view/KeyEvent"),
            jni::jni_sig!("(II)V"),
            &[JValue::Int(action), JValue::Int(key_code)],
        ) {
            Ok(o) => o,
            Err(_) => {
                env.exception_clear();
                return Ok(0);
            }
        };
        match env.call_method(
            &key_event,
            jni::jni_str!("getUnicodeChar"),
            jni::jni_sig!("(I)I"),
            &[JValue::Int(meta_state)],
        ) {
            Ok(v) => {
                let c = v.i().unwrap_or(0);
                Ok(if c > 0 { c as u32 } else { 0 })
            }
            Err(_) => {
                env.exception_clear();
                Ok(0)
            }
        }
    })
    .unwrap_or(0)
}

// ── public accessors ──────────────────────────────────────────────────────────

/// Public accessor for the JavaVM pointer.
///
/// Uses `AndroidApp::vm_as_ptr()` from the stored `AndroidApp`.
/// Used by `platform.rs` for JNI calls.
pub fn java_vm() -> *mut c_void {
    ANDROID_APP
        .get()
        .map(|app| app.vm_as_ptr())
        .unwrap_or(std::ptr::null_mut())
}

/// Public accessor for the current Activity's JNI object reference.
///
/// Uses `AndroidApp::activity_as_ptr()` from the stored `AndroidApp`.
/// Used by `platform.rs` and `window.rs` for JNI calls that require the
/// activity's jobject.
///
/// NOTE: This returns a jobject (JNI global ref), NOT an `ANativeActivity *`.
/// Code that previously used `(*activity).clazz` should use this directly
/// as the activity jobject.
pub fn activity_as_ptr() -> *mut c_void {
    ANDROID_APP
        .get()
        .map(|app| app.activity_as_ptr())
        .unwrap_or(std::ptr::null_mut())
}

/// Returns a clone of the stored `AndroidApp`, if initialised.
pub fn android_app() -> Option<AndroidApp> {
    ANDROID_APP.get().cloned()
}

/// Returns a reference to the global `AndroidPlatform`, if initialised.
///
/// Returns `None` before `android_main` has set it up.
pub fn platform() -> Option<&'static Arc<AndroidPlatform>> {
    PLATFORM.get()
}

/// Returns a [`SharedPlatform`] wrapping the global `Arc<AndroidPlatform>`.
///
/// This is the value you hand to `Application::with_platform(...)`:
///
/// ```rust,no_run
/// let platform = jni::shared_platform().unwrap();
/// Application::with_platform(platform.into_rc()).run(|cx| { … });
/// ```
///
/// Returns `None` before `init_platform` has been called.
pub fn shared_platform() -> Option<SharedPlatform> {
    PLATFORM
        .get()
        .map(|arc| SharedPlatform::new(Arc::clone(arc)))
}

// ── input event types ─────────────────────────────────────────────────────────

/// Motion event action constants from the NDK.
const AMOTION_EVENT_ACTION_DOWN: u32 = 0;
const AMOTION_EVENT_ACTION_UP: u32 = 1;
const AMOTION_EVENT_ACTION_MOVE: u32 = 2;
const AMOTION_EVENT_ACTION_CANCEL: u32 = 3;

// ── night mode query via NDK Configuration ───────────────────────────────────

/// Query the current night mode using the NDK Configuration API.
///
/// Returns `true` if the system is in dark mode.
pub fn query_night_mode_via_jni() -> bool {
    let app = match android_app() {
        Some(app) => app,
        None => return false,
    };

    // Build an ndk::configuration::Configuration from the app's asset manager.
    let config = ndk::configuration::Configuration::from_asset_manager(&app.asset_manager());
    let is_dark = config.ui_mode_night() == ndk::configuration::UiModeNight::Yes;

    log::debug!("query_night_mode (ndk): is_dark={}", is_dark);
    is_dark
}

// ── input event processing ────────────────────────────────────────────────────

/// Process input events from the `AndroidApp` and dispatch them to the window.
fn process_input_events(app: &AndroidApp) {
    let platform = match PLATFORM.get() {
        Some(p) => p,
        None => {
            log::trace!("process_input_events: no platform yet");
            return;
        }
    };

    let win = match platform.primary_window() {
        Some(w) => w,
        None => {
            log::trace!("process_input_events: no primary window yet");
            return;
        }
    };

    match app.input_events_iter() {
        Ok(mut iter) => {
            loop {
                let read_input = iter.next(|event| {
                    use android_activity::input::{InputEvent, MotionAction};

                    match event {
                        InputEvent::MotionEvent(motion_event) => {
                            let action = motion_event.action();
                            let pointer_count = motion_event.pointer_count();
                            // Cancellation belongs to the existing stream even over a native view,
                            // and may arrive without a usable pointer array.
                            if action == MotionAction::Cancel {
                                let native = NATIVE_TOUCH_ROUTE.with(|route| route.borrow_mut().route(false, true, || false));
                                if native { return android_activity::InputStatus::Unhandled; }
                                win.handle_touch(crate::android::TouchPoint { id: 0, x: 0.0, y: 0.0, action: AMOTION_EVENT_ACTION_CANCEL });
                                return android_activity::InputStatus::Handled;
                            }

                            log::debug!(
                                "process_input_events: MotionEvent action={:?} pointers={}",
                                action,
                                pointer_count,
                            );

                            // Check if this touch lands on a platform view.
                            //
                            // On Android with NativeActivity, ALL touch events go
                            // to the native surface first. Platform views are real
                            // Java Views in a FrameLayout overlay, but they won't
                            // receive touches unless we skip GPUI dispatch and let
                            // Android's view hierarchy handle the event instead.
                            //
                            // We use the primary pointer for the hit-test on DOWN
                            // actions. Physical pixel coordinates are converted to
                            // logical pixels to match PlatformViewBounds.
                            let hits_platform_view = NATIVE_TOUCH_ROUTE.with(|route| {
                                route.borrow_mut().route(
                                    action == MotionAction::Down,
                                    action == MotionAction::Up,
                                    || {
                                        if pointer_count == 0 {
                                            return false;
                                        }
                                        let registry = crate::platform_view::PlatformViewRegistry::global();
                                        if registry.active_view_count() == 0 {
                                            return false;
                                        }
                                        let primary = motion_event.pointer_at_index(0);
                                        let scale = win.scale_factor();
                                        registry.hit_test(primary.x() / scale, primary.y() / scale)
                                    },
                                )
                            });

                            if hits_platform_view {
                                log::debug!(
                                    "process_input_events: touch hits platform view, skipping GPUI dispatch",
                                );
                                // Return Unhandled so android-activity can pass
                                // the event back to the Java view hierarchy where
                                // the platform view's FrameLayout lives.
                                return android_activity::InputStatus::Unhandled;
                            }

                            for i in 0..pointer_count {
                                let pointer = motion_event.pointer_at_index(i);

                                let touch_action = match action {
                                    MotionAction::Down => AMOTION_EVENT_ACTION_DOWN,
                                    MotionAction::PointerDown => {
                                        // For pointer down, only dispatch the specific pointer
                                        if i != motion_event.pointer_index() {
                                            continue;
                                        }
                                        AMOTION_EVENT_ACTION_DOWN
                                    }
                                    MotionAction::Up => AMOTION_EVENT_ACTION_UP,
                                    MotionAction::PointerUp => {
                                        // For pointer up, only dispatch the specific pointer
                                        if i != motion_event.pointer_index() {
                                            continue;
                                        }
                                        AMOTION_EVENT_ACTION_UP
                                    }
                                    MotionAction::Move => AMOTION_EVENT_ACTION_MOVE,
                                    MotionAction::Cancel => AMOTION_EVENT_ACTION_CANCEL,
                                    _ => continue,
                                };

                                let touch = crate::android::TouchPoint {
                                    id: pointer.pointer_id(),
                                    x: pointer.x(),
                                    y: pointer.y(),
                                    action: touch_action,
                                };

                                log::debug!(
                                    "process_input_events: dispatching touch id={} x={:.0} y={:.0} action={}",
                                    touch.id, touch.x, touch.y, touch.action,
                                );

                                win.handle_touch(touch);
                            }

                            android_activity::InputStatus::Handled
                        }
                        InputEvent::KeyEvent(key_event) => {
                            use android_activity::input::KeyAction;

                            let action = match key_event.action() {
                                KeyAction::Down => 0,
                                KeyAction::Up => 1,
                                _ => return android_activity::InputStatus::Unhandled,
                            };

                            let key_code: u32 = key_event.key_code().into();
                            let meta_state: u32 = key_event.meta_state().0;

                            let unicode_char = unicode_char_for_key_event(
                                key_code as i32,
                                action,
                                meta_state as i32,
                            );

                            if unicode_char != 0 {
                                log::trace!(
                                    "dispatch_key_event: code={} action={} meta={:#x} → unicode=U+{:04X}",
                                    key_code,
                                    action,
                                    meta_state,
                                    unicode_char
                                );
                            }

                            let key_event = crate::android::AndroidKeyEvent {
                                key_code: key_code as i32,
                                action,
                                meta_state: meta_state as i32,
                                unicode_char,
                            };

                            win.handle_key_event(key_event);
                            android_activity::InputStatus::Handled
                        }
                        _ => android_activity::InputStatus::Unhandled,
                    }
                });

                if !read_input {
                    break;
                }
            }
        }
        Err(err) => {
            log::error!("Failed to get input events iterator: {err:?}");
        }
    }
}

// ── main event loop ───────────────────────────────────────────────────────────

/// The event loop that processes `android-activity` events and drives the
/// platform.
///
/// Called from `android_main` after the platform is initialised.
/// Runs until the platform requests quit or the activity is destroyed.
pub fn run_event_loop(app: &AndroidApp) {
    app.run_on_java_main_thread(Box::new(|| {
        if let Err(error) = with_env(|env| {
            let class = find_app_class(env, "dev.gpui.mobile.GpuiInsets")?;
            let activity = activity(env)?;
            env.call_static_method(
                &class,
                jni::jni_str!("install"),
                jni::jni_sig!("(Landroid/app/Activity;)V"),
                &[JValue::Object(&activity)],
            )
            .map(|_| ())
            .map_err(|error| {
                env.exception_clear();
                error.to_string()
            })
        }) {
            log::error!("WindowInsets observer installation failed: {error}");
        }
    }));
    log::info!("run_event_loop: entering main loop");

    // Track whether the on_init_window callback has already been invoked.
    // We do NOT invoke it inside handle_main_event (which runs inside
    // poll_events) because the callback can be heavy (shader compilation,
    // GPUI Application setup).  Running it there blocks the event loop and
    // prevents the system's FocusEvent from being consumed, triggering an
    // ANR after 10 seconds.
    //
    // Instead we check each loop iteration: if a primary window exists and
    // the callback is still pending, invoke it *after* poll_events has
    // returned so focus/input events have already been drained.
    INIT_WINDOW_DONE.store(false, Ordering::Relaxed);
    let mut iteration: u64 = 0;
    let mut last_heartbeat = std::time::Instant::now();
    let mut app_is_active = false;

    loop {
        iteration += 1;

        // Log a heartbeat every 5 seconds so we can tell if the loop is alive.
        let now = std::time::Instant::now();
        if now.duration_since(last_heartbeat) >= Duration::from_secs(5) {
            log::info!(
                "run_event_loop: heartbeat — iteration={}, init_done={}, active={}",
                iteration,
                INIT_WINDOW_DONE.load(Ordering::Relaxed),
                app_is_active,
            );
            last_heartbeat = now;
        }

        // Check if quit was requested.
        if let Some(platform) = PLATFORM.get() {
            if platform.should_quit() {
                log::info!("run_event_loop: platform requested quit");
                platform.on_thermal_host_destroyed();
                dispose_all_platform_views();
                break;
            }
            platform.tick();
        }

        // ── Poll for events (non-blocking) ──
        //
        // Non-blocking poll: process any pending events then immediately
        // continue to rendering. No sleep — the GPU present call
        // (get_current_texture / Mailbox) provides natural frame pacing.
        app.poll_events(Some(Duration::ZERO), |event| match event {
            PollEvent::Main(main_event) => {
                handle_main_event(app, main_event);
            }
            PollEvent::Wake => {}
            PollEvent::Timeout => {}
            _ => {}
        });

        // ── Deferred lifecycle processing ──
        //
        // Between each handler, call poll_events again to drain any
        // events the Java UI thread may have queued while we were
        // processing.  This keeps the condvar wait short and prevents
        // the InputDispatcher ANR (10s timeout on MotionEvent delivery).
        //
        // Helper closure to drain events quickly:
        let drain_events = |app: &AndroidApp| {
            app.poll_events(Some(Duration::ZERO), |event| {
                if let PollEvent::Main(main_event) = event {
                    handle_main_event(app, main_event);
                }
            });
        };

        // 1. TerminateWindow — unconfigure surface, release native window
        if TERM_WINDOW_PENDING.swap(false, Ordering::Relaxed) {
            log::info!("deferred: TerminateWindow (iter={})", iteration);
            INIT_WINDOW_DONE.store(false, Ordering::Relaxed);
            *LAST_CHROME_STYLE.lock().unwrap() = None;
            if let Some(platform) = PLATFORM.get() {
                if let Some(win) = platform.primary_window() {
                    win.term_window();
                }
            }
            // Drain immediately — Java thread may be blocked on InitWindow condvar.
            drain_events(app);
        }

        // 2. InitWindow — replace surface on existing renderer, or create new
        if INIT_WINDOW_PENDING.swap(false, Ordering::Relaxed) {
            log::info!("deferred: InitWindow (iter={})", iteration);
            if let Some(platform) = PLATFORM.get() {
                if let Some(native_window) = app.native_window() {
                    let width = native_window.width();
                    let height = native_window.height();
                    log::info!("InitWindow: {}×{}", width, height);

                    platform.update_primary_display(&native_window, &app.asset_manager());

                    let scale_factor = platform
                        .primary_display()
                        .map(|d| d.scale_factor())
                        .unwrap_or(1.0);

                    if let Some(existing) = platform.primary_window() {
                        let gpu_ctx = platform.gpu_context();
                        match existing.init_window(native_window, gpu_ctx) {
                            Ok(()) => {
                                log::info!("InitWindow: reinitialised existing window");
                            }
                            Err(e) => {
                                log::error!("failed to reinit window surface: {e:#}");
                            }
                        }

                        // Trigger GPUI resize so layout adapts to new dimensions.
                        existing.handle_resize();

                        let cr = app.content_rect();
                        existing.update_safe_area_from_content_rect(
                            cr.left, cr.top, cr.right, cr.bottom,
                        );

                        INIT_WINDOW_DONE.store(true, Ordering::Relaxed);
                    } else {
                        match platform.open_window(native_window, scale_factor, false) {
                            Ok(win) => {
                                log::info!(
                                    "window opened — id={:#x} scale={:.1}",
                                    win.id(),
                                    scale_factor
                                );

                                let cr = app.content_rect();
                                win.update_safe_area_from_content_rect(
                                    cr.left, cr.top, cr.right, cr.bottom,
                                );
                            }
                            Err(e) => {
                                log::error!("failed to open window: {e:#}");
                            }
                        }
                    }
                }
            }
            // Drain — Java thread may have queued ConfigChanged/WindowResized.
            drain_events(app);
        }

        // 3. WindowResized
        if WINDOW_RESIZED_PENDING.swap(false, Ordering::Relaxed) {
            log::debug!("deferred: WindowResized");
            if let Some(platform) = PLATFORM.get() {
                if let Some(win) = platform.primary_window() {
                    win.handle_resize();
                    let cr = app.content_rect();
                    win.update_safe_area_from_content_rect(cr.left, cr.top, cr.right, cr.bottom);
                }
            }
        }

        // 4. ConfigChanged
        if let Some(window) = PLATFORM
            .get()
            .and_then(|platform| platform.primary_window())
        {
            let pending = WINDOW_INSETS_PENDING.lock().take();
            if let Some((density, insets)) = pending {
                window.update_window_insets(density, insets);
            }
        }

        if CONFIG_CHANGED_PENDING.swap(false, Ordering::Relaxed) {
            log::debug!("deferred: ConfigChanged");
            if let Some(platform) = PLATFORM.get() {
                platform.notify_keyboard_layout_change();
                let is_dark = query_night_mode_via_jni();
                if let Some(win) = platform.primary_window() {
                    let appearance = if is_dark {
                        crate::android::window::WindowAppearance::Dark
                    } else {
                        crate::android::window::WindowAppearance::Light
                    };
                    win.set_appearance(appearance);
                }
            }
        }

        if let Some(win) = PLATFORM.get().and_then(|p| p.primary_window()) {
            let focus = std::mem::take(&mut *FOCUS_PENDING.lock().unwrap());
            for active in focus {
                win.set_active(active);
            }
        }

        // 5. Pause / background
        if PAUSE_PENDING.swap(false, Ordering::Relaxed) {
            log::info!("deferred: Pause (iter={})", iteration);
            if let Some(platform) = PLATFORM.get() {
                platform.did_enter_background();
                if let Some(win) = platform.primary_window() {
                    win.notify_app_lifecycle(false);
                }
            }
            // Pause platform views
            pause_platform_views();
        }

        // 6. Resume / foreground
        if RESUME_PENDING.swap(false, Ordering::Relaxed) {
            log::info!("deferred: Resume (iter={})", iteration);
            if let Some(platform) = PLATFORM.get() {
                platform.did_become_active();
                if let Some(win) = platform.primary_window() {
                    win.notify_app_lifecycle(true);
                }
            }
            // Resume platform views
            resume_platform_views();
        }

        // Track active/focused state.
        if let Some(platform) = PLATFORM.get() {
            if let Some(win) = platform.primary_window() {
                let is_active = win.is_active();
                if is_active != app_is_active {
                    log::info!(
                        "run_event_loop: active {} -> {} (iter={})",
                        app_is_active,
                        is_active,
                        iteration,
                    );
                    app_is_active = is_active;
                }
            }
        }

        // Process input events.
        process_input_events(app);

        // Deferred initialisation callbacks (runs once).
        if !INIT_WINDOW_DONE.load(Ordering::Relaxed) {
            if let Some(platform) = PLATFORM.get() {
                if platform.primary_window().is_some() {
                    if let Some(finish_cb) = platform.take_finish_launching_callback() {
                        log::info!("invoking finish_launching callback (iter={})", iteration);
                        finish_cb();
                    }

                    if let Some(init_cb) = platform.take_on_init_window_callback() {
                        let win = platform.primary_window().unwrap();
                        log::info!("invoking on_init_window callback (iter={})", iteration);
                        init_cb(win);
                    }

                    INIT_WINDOW_DONE.store(true, Ordering::Relaxed);
                    NATIVE_INITIALIZED.store(true, Ordering::Release);
                    log::info!("NATIVE_INITIALIZED = true");

                    // Render first frame immediately.
                    platform.flush_main_thread_tasks();
                    if let Some(win) = platform.primary_window() {
                        win.request_frame();
                    }
                }
            }
        }

        // Dispatch only after poll_events releases android-activity's locks.
        if INIT_WINDOW_DONE.load(Ordering::Relaxed) {
            drain_platform_events();
        }
        // ── Render ──
        if let Some(platform) = PLATFORM.get() {
            if INIT_WINDOW_DONE.load(Ordering::Relaxed) && app_is_active {
                platform.flush_main_thread_tasks();
                if let Some(win) = platform.primary_window() {
                    win.request_frame();
                }

                // Drain lifecycle events that arrived during rendering
                // (e.g. rotation triggers TerminateWindow while we were
                // in get_current_texture / present).
                drain_events(app);
                process_input_events(app);
            }
        }

        // Yield CPU to avoid starving system threads and causing ANR.
        // Keep this short — at 120Hz the frame budget is only 8.3ms.
        std::thread::sleep(Duration::from_micros(500));
    }

    log::info!("run_event_loop: exiting main loop");
}

/// Pause all platform views when the app goes to background.
fn pause_platform_views() {
    let _ = with_env(|env| {
        if let Ok(helper_class) = find_app_class(env, "dev.gpui.mobile.GpuiPlatformView") {
            let _ = env.call_static_method(
                &helper_class,
                jni::jni_str!("pauseAll"),
                jni::jni_sig!("()V"),
                &[],
            );
            env.exception_clear();
        }
        Ok(())
    });
}

/// Resume all platform views when the app returns to foreground.
fn resume_platform_views() {
    let _ = with_env(|env| {
        if let Ok(helper_class) = find_app_class(env, "dev.gpui.mobile.GpuiPlatformView") {
            let _ = env.call_static_method(
                &helper_class,
                jni::jni_str!("resumeAll"),
                jni::jni_sig!("()V"),
                &[],
            );
            env.exception_clear();
        }
        Ok(())
    });
}

/// Dispose all platform views during app shutdown.
fn dispose_all_platform_views() {
    let _ = with_env(|env| {
        if let Ok(helper_class) = find_app_class(env, "dev.gpui.mobile.GpuiPlatformView") {
            let _ = env.call_static_method(
                &helper_class,
                jni::jni_str!("disposeAll"),
                jni::jni_sig!("()V"),
                &[],
            );
            env.exception_clear();
        }
        Ok(())
    });
}

/// Handle a single `MainEvent` from `android-activity`.
fn handle_main_event(_app: &AndroidApp, event: MainEvent<'_>) {
    match event {
        MainEvent::InitWindow { .. } => {
            log::info!("MainEvent::InitWindow");
            // Defer to after poll_events to avoid deadlock with state lock.
            INIT_WINDOW_PENDING.store(true, Ordering::Relaxed);
        }

        MainEvent::TerminateWindow { .. } => {
            log::info!("MainEvent::TerminateWindow");
            // Defer to after poll_events to avoid deadlock with state lock.
            TERM_WINDOW_PENDING.store(true, Ordering::Relaxed);
        }

        MainEvent::WindowResized { .. } => {
            log::debug!("MainEvent::WindowResized");
            // Defer to after poll_events to avoid deadlock with state lock.
            WINDOW_RESIZED_PENDING.store(true, Ordering::Relaxed);
        }

        MainEvent::GainedFocus => {
            log::info!("MainEvent::GainedFocus");
            FOCUS_PENDING.lock().unwrap().push(true);
        }

        MainEvent::LostFocus => {
            log::info!("MainEvent::LostFocus");
            FOCUS_PENDING.lock().unwrap().push(false);
        }

        MainEvent::Resume { .. } => {
            queue_platform_event(Some(gpui::AppLifecyclePhase::Active));
            log::info!("MainEvent::Resume");
            RESUME_PENDING.store(true, Ordering::Relaxed);
        }

        MainEvent::Pause => {
            queue_platform_event(Some(gpui::AppLifecyclePhase::Inactive));
            log::info!("MainEvent::Pause");
            // set_active uses AtomicBool so it never blocks.
            PAUSE_PENDING.store(true, Ordering::Relaxed);
        }

        MainEvent::ConfigChanged { .. } => {
            log::debug!("MainEvent::ConfigChanged");
            CONFIG_CHANGED_PENDING.store(true, Ordering::Relaxed);
        }

        MainEvent::Start => {
            queue_platform_event(Some(gpui::AppLifecyclePhase::Foreground));
            log::info!("MainEvent::Start");
        }

        MainEvent::Stop => {
            queue_platform_event(Some(gpui::AppLifecyclePhase::Background));
            log::info!("MainEvent::Stop");
        }

        MainEvent::SaveState { .. } => {
            log::info!("MainEvent::SaveState");
        }

        MainEvent::LowMemory => {
            queue_platform_event(None);
            log::warn!("MainEvent::LowMemory — consider releasing cached resources");
        }

        MainEvent::Destroy => {
            log::info!("MainEvent::Destroy");
            THERMAL_GENERATION.store(0, Ordering::Release);
            THERMAL_STOP_PENDING.store(true, Ordering::Release);

            if let Some(platform) = PLATFORM.get() {
                platform.quit();
            }
        }

        MainEvent::InsetsChanged { .. } => {
            log::debug!("MainEvent::InsetsChanged");
            WINDOW_RESIZED_PENDING.store(true, Ordering::Relaxed);
        }

        MainEvent::ContentRectChanged { .. } => {
            log::debug!("MainEvent::ContentRectChanged");
            WINDOW_RESIZED_PENDING.store(true, Ordering::Relaxed);
        }

        _ => {
            log::trace!("MainEvent: other variant");
        }
    }
}

// ── main loop helper (compat with existing code) ──────────────────────────────

/// Run one iteration of the event loop.
///
/// This is a compatibility wrapper for code that uses a manual poll loop.
/// Prefer `run_event_loop` for the standard event loop.
///
/// `timeout_ms` — how long to block waiting for events (milliseconds).
/// Pass `0` for non-blocking, `-1` to block indefinitely.
///
/// Returns `true` if the application should exit.
pub fn poll_events(timeout_ms: i32) -> bool {
    if let Some(platform) = PLATFORM.get() {
        if platform.should_quit() {
            platform.on_thermal_host_destroyed();
            return true;
        }
        platform.tick();
    }

    let app = match ANDROID_APP.get() {
        Some(app) => app,
        None => return false,
    };

    let timeout = if timeout_ms < 0 {
        None
    } else {
        Some(Duration::from_millis(timeout_ms as u64))
    };

    app.poll_events(timeout, |event| match event {
        PollEvent::Main(main_event) => {
            handle_main_event(app, main_event);
        }
        PollEvent::Wake => {}
        _ => {}
    });

    drain_platform_events();
    process_input_events(app);

    // Drive the GPUI rendering pipeline (same as run_event_loop).
    if let Some(platform) = PLATFORM.get() {
        platform.flush_main_thread_tasks();
        if let Some(win) = platform.primary_window() {
            win.request_frame();
        }
    }

    false
}

// ── public init / run helpers ─────────────────────────────────────────────────

/// Install a panic hook that routes panic messages to logcat.
///
/// Call this early in `android_main` so that any subsequent panic is
/// visible via `adb logcat`.  Safe to call multiple times — each call
/// replaces the previous hook.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Box<dyn Any>".to_string()
        };
        if let Some(loc) = info.location() {
            log::error!(
                "PANIC at {}:{}:{}: {}",
                loc.file(),
                loc.line(),
                loc.column(),
                payload
            );
        } else {
            log::error!("PANIC: {}", payload);
        }
    }));
}

/// Store the `AndroidApp` globally and create the `AndroidPlatform`.
///
/// Must be called exactly once from `android_main` before
/// `run_event_loop`.  Returns a reference to the platform so the caller
/// can register callbacks (e.g. `set_on_init_window`) before entering
/// the event loop.
pub fn init_platform(app: &AndroidApp) -> &'static Arc<AndroidPlatform> {
    let _ = ANDROID_APP.set(app.clone());
    log::info!("init_platform: stored AndroidApp");

    let platform = Arc::new(AndroidPlatform::new(false));
    log::info!("init_platform: AndroidPlatform created");

    PLATFORM
        .set(Arc::clone(&platform))
        .unwrap_or_else(|_| log::warn!("PLATFORM already set — duplicate init_platform?"));

    // SAFETY: we just set it above.
    PLATFORM.get().unwrap()
}

// ── system chrome (status bar / navigation bar) ───────────────────────────────

/// Cached last-applied system chrome style.
///
/// `set_system_chrome` is called on every frame render.  The JNI calls it
/// makes (getWindow, setStatusBarColor, etc.) are View operations that can
/// contend with the Android UI thread and intermittently deadlock.
/// By caching the last applied style we skip the JNI calls entirely when
/// nothing changed — which is the common case.
#[allow(clippy::type_complexity)]
static LAST_CHROME_STYLE: std::sync::Mutex<
    Option<(Option<u32>, Option<u32>, crate::StatusBarContentStyle)>,
> = std::sync::Mutex::new(None);

/// Apply system chrome styling on Android.
///
/// Sets the status bar color, navigation bar color, and light/dark
/// status bar icons via JNI calls to `Window` and `WindowInsetsController`.
///
/// Must be called from the main (native) thread that has JNI access.
pub fn set_system_chrome(style: &crate::SystemChromeStyle) {
    let status_bar_color = style.status_bar_color;
    let navigation_bar_color = style.navigation_bar_color;
    let status_bar_style = style.status_bar_style;

    // Skip the (expensive, potentially deadlocking) JNI calls when nothing changed.
    {
        let key = (status_bar_color, navigation_bar_color, status_bar_style);
        let mut last = LAST_CHROME_STYLE.lock().unwrap();
        if *last == Some(key) {
            return;
        }
        *last = Some(key);
    }

    let result = with_env(|env| {
        let activity_obj = activity(env)?;

        // 1. Get the Window: activity.getWindow()
        let window = env
            .call_method(
                &activity_obj,
                jni::jni_str!("getWindow"),
                jni::jni_sig!("()Landroid/view/Window;"),
                &[],
            )
            .and_then(|v: jni::objects::JValueOwned| v.l())
            .map_err(|e| {
                env.exception_clear();
                e.to_string()
            })?;
        if window.is_null() {
            return Err("getWindow returned null".into());
        }

        // 2. Set status bar color if provided
        if let Some(color) = status_bar_color {
            let argb = (0xFF000000_u32 | color) as i32;
            let _ = env.call_method(
                &window,
                jni::jni_str!("setStatusBarColor"),
                jni::jni_sig!("(I)V"),
                &[JValue::Int(argb)],
            );
            env.exception_clear();
        }

        // 3. Set navigation bar color if provided
        if let Some(color) = navigation_bar_color {
            let argb = (0xFF000000_u32 | color) as i32;
            let _ = env.call_method(
                &window,
                jni::jni_str!("setNavigationBarColor"),
                jni::jni_sig!("(I)V"),
                &[JValue::Int(argb)],
            );
            env.exception_clear();
        }

        // 4. Set light/dark status bar icons via WindowInsetsController (API 30+)
        let insetsctl = env.call_method(
            &window,
            jni::jni_str!("getInsetsController"),
            jni::jni_sig!("()Landroid/view/WindowInsetsController;"),
            &[],
        );

        if let Ok(v) = insetsctl {
            if let Ok(ctl) = v.l() {
                if !ctl.is_null() {
                    let mask: i32 = 0x00000008;
                    let appearance: i32 = match status_bar_style {
                        crate::StatusBarContentStyle::Dark => 0x00000008,
                        crate::StatusBarContentStyle::Light => 0,
                    };
                    let _ = env.call_method(
                        &ctl,
                        jni::jni_str!("setSystemBarsAppearance"),
                        jni::jni_sig!("(II)V"),
                        &[JValue::Int(appearance), JValue::Int(mask)],
                    );
                    env.exception_clear();
                }
            }
        } else {
            env.exception_clear();

            if let Ok(decor) = env
                .call_method(
                    &window,
                    jni::jni_str!("getDecorView"),
                    jni::jni_sig!("()Landroid/view/View;"),
                    &[],
                )
                .and_then(|v: jni::objects::JValueOwned| v.l())
            {
                if !decor.is_null() {
                    if let Ok(current) = env
                        .call_method(
                            &decor,
                            jni::jni_str!("getSystemUiVisibility"),
                            jni::jni_sig!("()I"),
                            &[],
                        )
                        .and_then(|v: jni::objects::JValueOwned| v.i())
                    {
                        let new_flags = match status_bar_style {
                            crate::StatusBarContentStyle::Dark => current | 0x00002000,
                            crate::StatusBarContentStyle::Light => current & !0x00002000,
                        };
                        let _ = env.call_method(
                            &decor,
                            jni::jni_str!("setSystemUiVisibility"),
                            jni::jni_sig!("(I)V"),
                            &[JValue::Int(new_flags)],
                        );
                        env.exception_clear();
                    }
                }
            }
        }

        Ok(())
    });

    if let Err(e) = result {
        log::warn!("set_system_chrome: {e}");
    }

    log::info!(
        "set_system_chrome: status_bar_color={:?}, nav_bar_color={:?}, style={:?}",
        style.status_bar_color,
        style.navigation_bar_color,
        style.status_bar_style
    );
}

// ── software keyboard (IME) control ───────────────────────────────────────────

type PromptSender = futures::channel::oneshot::Sender<usize>;
static PROMPTS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<i64, PromptSender>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
static NEXT_PROMPT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

pub(crate) fn show_prompt(
    message: &str,
    detail: Option<&str>,
    answers: &[gpui::PromptButton],
) -> Option<futures::channel::oneshot::Receiver<usize>> {
    let app = android_app()?;
    if answers.is_empty() || answers.len() > i32::MAX as usize {
        return None;
    }
    let message = message.to_owned();
    let detail = detail.unwrap_or_default().to_owned();
    let labels: Vec<String> = answers
        .iter()
        .map(|answer| answer.label().to_string())
        .collect();
    let cancel = answers
        .iter()
        .position(|answer| answer.is_cancel())
        .map_or(-1, |index| index as i32);
    let id = NEXT_PROMPT.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = futures::channel::oneshot::channel();
    PROMPTS.lock().unwrap().insert(id, sender);
    app.run_on_java_main_thread(Box::new(move || {
        let result = with_env(|env| {
            let class = find_app_class(env, "dev.gpui.mobile.GpuiPrompt")?;
            let activity = activity(env)?;
            let message = env.new_string(message).map_err(|e| e.to_string())?;
            let detail = env.new_string(detail).map_err(|e| e.to_string())?;
            let string_class = env.find_class(jni::jni_str!("java/lang/String")).map_err(|e| e.to_string())?;
            let array = env.new_object_array(labels.len() as i32, &string_class, JObject::null()).map_err(|e| e.to_string())?;
            for (index, label) in labels.iter().enumerate() {
                let label = env.new_string(label).map_err(|e| e.to_string())?;
                env.set_object_array_element(&array, index, &label).map_err(|e| e.to_string())?;
            }
            env.call_static_method(&class, jni::jni_str!("show"),
                jni::jni_sig!("(Landroid/app/Activity;JLjava/lang/String;Ljava/lang/String;[Ljava/lang/String;I)V"),
                &[JValue::Object(&activity), JValue::Long(id), JValue::Object(&message), JValue::Object(&detail), JValue::Object(&array), JValue::Int(cancel)])
                .map(|_| ()).map_err(|e| { env.exception_clear(); e.to_string() })
        });
        if let Err(error) = result {
            PROMPTS.lock().unwrap().remove(&id);
            log::warn!("Android prompt failed: {error}");
        }
    }));
    Some(receiver)
}

/// Java only completes a channel; user callbacks run on their executor, outside this lock.
#[unsafe(no_mangle)]
pub extern "C" fn Java_dev_gpui_mobile_GpuiPrompt_complete(
    _env: *mut c_void,
    _class: *mut c_void,
    id: i64,
    answer: i32,
) {
    let sender = PROMPTS.lock().unwrap().remove(&id);
    if let Some(sender) = sender {
        if answer >= 0 {
            let _ = sender.send(answer as usize);
        }
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    #[test]
    fn prompt_completion_is_once_and_cancellation_disconnects() {
        let id = NEXT_PROMPT.fetch_add(1, Ordering::Relaxed);
        let (sender, mut receiver) = futures::channel::oneshot::channel();
        PROMPTS.lock().unwrap().insert(id, sender);
        Java_dev_gpui_mobile_GpuiPrompt_complete(std::ptr::null_mut(), std::ptr::null_mut(), id, 2);
        Java_dev_gpui_mobile_GpuiPrompt_complete(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            id,
            -1,
        );
        assert_eq!(receiver.try_recv().unwrap(), Some(2));
        let id = NEXT_PROMPT.fetch_add(1, Ordering::Relaxed);
        let (sender, mut receiver) = futures::channel::oneshot::channel();
        PROMPTS.lock().unwrap().insert(id, sender);
        Java_dev_gpui_mobile_GpuiPrompt_complete(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            id,
            -1,
        );
        assert!(receiver.try_recv().is_err());
    }
}

/// Show the software keyboard on Android.
///
/// `android-activity` accesses InputMethodManager and Views through JNI on the
/// calling thread, so schedule the operation on Java's main thread explicitly.
pub fn show_keyboard_android(keyboard_type: crate::KeyboardType) {
    let (flags, options) = IME_CONFIGURATION
        .with(|configuration| crate::mobile_events::android_input_traits(&configuration.borrow()));
    let key = (
        keyboard_type,
        flags,
        options,
        IME_EDITOR.load(Ordering::Acquire),
    );
    if IME_LOGICAL_OWNER.load(Ordering::Acquire)
        && IME_ACTIVE.load(Ordering::Acquire)
        && IME_SHOWN_KEY.get() == Some(key)
    {
        return;
    }
    IME_SHOWN_KEY.set(Some(key));
    IME_TYPE.set(keyboard_type);
    IME_ACTIVE.store(true, Ordering::Release);
    IME_LEGACY_ENABLED.store(
        crate::mobile_dispatch::has_text_handler(),
        Ordering::Release,
    );
    let session = IME_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    IME_LEGACY_GENERATION.set(crate::mobile_dispatch::text_handler_generation());
    if let Some(app) = android_app() {
        app.run_on_java_main_thread(Box::new(move || {
            if IME_GENERATION.load(Ordering::Acquire) != session {
                return;
            }
            let result = with_env(|env| {
                let class = find_app_class(env, "dev.gpui.mobile.GpuiKeyboard")?;
                let activity = activity(env)?;
                let mut input_type = match keyboard_type {
                    crate::KeyboardType::Default => 1,
                    crate::KeyboardType::EmailAddress => 0x21,
                    crate::KeyboardType::Phone => 3,
                    crate::KeyboardType::NumberPad => 2,
                    crate::KeyboardType::URL => 0x11,
                    crate::KeyboardType::Decimal => 0x2002,
                };
                if input_type & 0xf == 1 {
                    input_type |= flags;
                }
                env.call_static_method(
                    &class,
                    jni::jni_str!("show"),
                    jni::jni_sig!("(Landroid/app/Activity;IIJ)V"),
                    &[
                        JValue::Object(&activity),
                        JValue::Int(input_type),
                        JValue::Int(options),
                        JValue::Long(session),
                    ],
                )
                .map(|_| ())
                .map_err(|error| {
                    env.exception_clear();
                    error.to_string()
                })
            });
            if let Err(error) = result {
                log::warn!("show_keyboard_android: {error}");
            }
        }));
    }
}

/// Hide the software keyboard on Android.
///
/// Schedule JNI InputMethodManager/View access on Java's main thread.
pub fn hide_keyboard_android() {
    IME_ACTIVE.store(false, Ordering::Release);
    IME_HAS_HANDLER.store(false, Ordering::Release);
    IME_LEGACY_ENABLED.store(false, Ordering::Release);
    let session = IME_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    if let Some(app) = android_app() {
        app.run_on_java_main_thread(Box::new(move || {
            if IME_GENERATION.load(Ordering::Acquire) != session {
                return;
            }
            let result = with_env(|env| {
                let class = find_app_class(env, "dev.gpui.mobile.GpuiKeyboard")?;
                let activity = activity(env)?;
                env.call_static_method(
                    &class,
                    jni::jni_str!("hide"),
                    jni::jni_sig!("(Landroid/app/Activity;)V"),
                    &[JValue::Object(&activity)],
                )
                .map(|_| ())
                .map_err(|error| {
                    env.exception_clear();
                    error.to_string()
                })
            });
            if let Err(error) = result {
                log::warn!("hide_keyboard_android: {error}");
            }
        }));
    }
}

static IME_TEXT: std::sync::Mutex<Vec<(i64, i64, crate::mobile_events::ImeInput)>> =
    std::sync::Mutex::new(Vec::new());
static IME_GENERATION: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static IME_ACTIVE: AtomicBool = AtomicBool::new(false);
pub(crate) static IME_HAS_HANDLER: AtomicBool = AtomicBool::new(false);
static IME_LEGACY_ENABLED: AtomicBool = AtomicBool::new(false);
pub(crate) static IME_EDITOR: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static IME_LOGICAL_OWNER: AtomicBool = AtomicBool::new(false);

pub(crate) fn logical_input_owner_changed(active: bool) {
    IME_LOGICAL_OWNER.store(active, Ordering::Release);
    IME_EDITOR.fetch_add(1, Ordering::AcqRel);
    if active {
        show_keyboard_android(IME_TYPE.get());
    } else {
        hide_keyboard_android();
    }
}

pub(crate) fn input_handler_changed() {
    if crate::mobile_events::active_text_input_lease().is_none() {
        IME_EDITOR.fetch_add(1, Ordering::AcqRel);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_dev_gpui_mobile_GpuiKeyboard_hasLogicalOwner(
    _env: *mut c_void,
    _class: *mut c_void,
) -> u8 {
    IME_LOGICAL_OWNER.load(Ordering::Acquire) as u8
}
thread_local! {
    static IME_SHOWN_KEY: std::cell::Cell<Option<(crate::KeyboardType, i32, i32, i64)>> = const { std::cell::Cell::new(None) };
    static IME_CONFIGURATION: std::cell::RefCell<gpui::TextInputConfiguration> = Default::default();
    static IME_TYPE: std::cell::Cell<crate::KeyboardType> = const { std::cell::Cell::new(crate::KeyboardType::Default) };
    static IME_LEGACY_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
pub(crate) fn ime_session_current(session: i64) -> bool {
    IME_ACTIVE.load(Ordering::Acquire)
        && IME_GENERATION.load(Ordering::Acquire) == session
        && (IME_LOGICAL_OWNER.load(Ordering::Acquire)
            || IME_LEGACY_GENERATION.get() == crate::mobile_dispatch::text_handler_generation())
}
pub(crate) fn ime_delivery_current(session: i64, editor: i64) -> bool {
    ime_session_current(session) && IME_EDITOR.load(Ordering::Acquire) == editor
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_dev_gpui_mobile_GpuiKeyboard_editorEpoch(
    _env: *mut c_void,
    _class: *mut c_void,
) -> i64 {
    IME_EDITOR.load(Ordering::Acquire)
}
pub(crate) fn configure_text_input(configuration: gpui::TextInputConfiguration) {
    let changed = IME_CONFIGURATION.with(|stored| {
        if *stored.borrow() == configuration {
            return false;
        }
        *stored.borrow_mut() = configuration;
        true
    });
    if changed && IME_ACTIVE.load(Ordering::Acquire) {
        show_keyboard_android(IME_TYPE.get());
    }
}
static OPEN_URLS_PENDING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Copy the JNI string now; dispatch only on the native event-loop thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_dev_gpui_mobile_GpuiKeyboard_committed(
    _env: *mut c_void,
    _class: *mut c_void,
    session: i64,
    editor: i64,
    action: i32,
    text: *mut c_void,
) {
    let _ = with_env(|env| {
        let text = unsafe { JObject::from_raw(env, text as jni::sys::jobject) };
        let text = get_string(env, &text);
        log::info!("IME JNI commit session={session} editor={editor} action={action}");
        let Some(input) = crate::mobile_events::ime_action(action, text) else {
            return Ok(());
        };
        IME_TEXT.lock().unwrap().push((session, editor, input));
        if let Some(app) = ANDROID_APP.get() {
            app.create_waker().wake();
        }
        Ok(())
    });
}

/// Marked updates preserve their originating connection/owner; never become commits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_dev_gpui_mobile_GpuiKeyboard_composed(
    _env: *mut c_void,
    _class: *mut c_void,
    session: i64,
    editor: i64,
    text: *mut c_void,
    start: i32,
    end: i32,
) {
    let _ = with_env(|env| {
        let object = unsafe { JObject::from_raw(env, text as jni::sys::jobject) };
        let text = get_string(env, &object);
        let len = text.encode_utf16().count();
        if start < 0 || end < start || end as usize > len {
            return Ok(());
        }
        IME_TEXT.lock().unwrap().push((
            session,
            editor,
            crate::mobile_events::ImeInput::Compose {
                text,
                selection: start as usize..end as usize,
            },
        ));
        if let Some(app) = ANDROID_APP.get() {
            app.create_waker().wake();
        }
        Ok(())
    });
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Queue an explicitly unit-tagged backward deletion; no Java references escape.
#[unsafe(no_mangle)]
pub extern "C" fn Java_dev_gpui_mobile_GpuiKeyboard_deleteBefore(
    _env: *mut c_void,
    _class: *mut c_void,
    session: i64,
    editor: i64,
    count: i32,
    codepoints: u8,
) -> u8 {
    if count < 0
        || !IME_ACTIVE.load(Ordering::Acquire)
        || IME_GENERATION.load(Ordering::Acquire) != session
        || IME_EDITOR.load(Ordering::Acquire) != editor
    {
        return 0;
    }
    if count == 0 {
        return 1;
    }
    let Some(input) = crate::mobile_events::ime_delete_request(
        count as usize,
        codepoints != 0,
        IME_HAS_HANDLER.load(Ordering::Acquire),
        IME_LEGACY_ENABLED.load(Ordering::Acquire),
    ) else {
        return 0;
    };
    IME_TEXT.lock().unwrap().push((session, editor, input));
    if let Some(app) = ANDROID_APP.get() {
        app.create_waker().wake();
    }
    1
}

// ── JNI export for GpuiActivity splash screen ────────────────────────────────

/// Called from `GpuiActivity.nativeIsInitialized()` to check whether the
/// native library has finished initializing (first frame rendered).
///
/// The AndroidX SplashScreen API calls this via `setKeepOnScreenCondition`
/// to hold the splash visible until GPUI is ready.
///
/// # Safety
/// Must only be called from the JVM on a valid JNI thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_dev_gpui_mobile_GpuiActivity_nativeIsInitialized(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
) -> u8 {
    if NATIVE_INITIALIZED.load(Ordering::Acquire) {
        1 // JNI_TRUE
    } else {
        0 // JNI_FALSE
    }
}

/// JNI bridge: receive a deeplink URL from `GpuiActivity.onNewIntent()`.
///
/// When the app is already running and a deeplink is opened (e.g. via
/// `adb shell am start -d gpui://video_player`), the Java side calls
/// this to notify the Rust deeplink handler.
///
/// # Safety
/// Must only be called from the JVM on a valid JNI thread with a valid `url` jobject.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_dev_gpui_mobile_GpuiActivity_nativeOnDeepLink(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
    url: *mut std::ffi::c_void,
) {
    // We already have a JVM attached on this thread (UI thread).
    // Use with_env to get a properly wrapped Env handle.
    let url_raw = url as jni::sys::jobject;
    let _ = with_env(|env| {
        let url_obj = unsafe { JObject::from_raw(env, url_raw) };
        let url_string = get_string(env, &url_obj);
        // Don't let the JObject be dropped (it's owned by the JNI call frame).
        if url_string.is_empty() {
            return Ok(());
        }
        log::info!("nativeOnDeepLink: {}", url_string);
        OPEN_URLS_PENDING.lock().unwrap().push(url_string.clone());
        if let Some(app) = ANDROID_APP.get() {
            app.create_waker().wake();
        }

        #[cfg(feature = "deeplink")]
        {
            crate::packages::deeplink::notify_deep_link(&url_string);
        }
        Ok(())
    });
}

/// JNI bridge: receive a media action from `GpuiMediaSession` system controls.
///
/// Actions: "play", "pause", "stop", "next", "previous"
///
/// # Safety
/// Must only be called from the JVM on a valid JNI thread with a valid `action` jobject.
#[cfg(feature = "media_session")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_dev_gpui_mobile_GpuiMediaSession_nativeMediaAction(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
    action: *mut std::ffi::c_void,
) {
    let action_raw = action as jni::sys::jobject;
    let _ = with_env(|env| {
        let action_obj = unsafe { JObject::from_raw(env, action_raw) };
        let action_str = get_string(env, &action_obj);

        let media_action = match action_str.as_str() {
            "play" => crate::packages::media_session::MediaAction::Play,
            "pause" => crate::packages::media_session::MediaAction::Pause,
            "stop" => crate::packages::media_session::MediaAction::Stop,
            "next" => crate::packages::media_session::MediaAction::Next,
            "previous" => crate::packages::media_session::MediaAction::Previous,
            other => {
                log::warn!("nativeMediaAction: unknown action '{}'", other);
                return Ok(());
            }
        };

        log::info!("nativeMediaAction: {:?}", media_action);
        crate::packages::media_session::notify_action(media_action);
        Ok(())
    });
}

/// JNI bridge: receive a seek request from `GpuiMediaSession` system controls.
///
/// # Safety
/// Must only be called from the JVM on a valid JNI thread.
#[cfg(feature = "media_session")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_dev_gpui_mobile_GpuiMediaSession_nativeMediaSeek(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
    position_ms: i64,
) {
    log::info!("nativeMediaSeek: {}ms", position_ms);
    crate::packages::media_session::notify_seek(position_ms as u64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_events_returns_false_when_no_platform() {
        // PLATFORM is not set in a unit-test context, so poll_events should
        // be a safe no-op and return false (don't quit).
        let result = poll_events(0);
        let _ = result;
    }

    #[test]
    fn java_vm_returns_null_before_init() {
        // Before android_main is called, java_vm() should return null.
        let vm = java_vm();
        assert!(vm.is_null());
    }

    #[test]
    fn activity_as_ptr_returns_null_before_init() {
        let ptr = activity_as_ptr();
        assert!(ptr.is_null());
    }

    #[test]
    fn android_app_returns_none_before_init() {
        assert!(android_app().is_none());
    }

    #[test]
    fn platform_returns_none_before_init() {
        assert!(platform().is_none());
    }
}
