//! Android window implementation.
//!
//! `AndroidWindow` wraps an `ANativeWindow *` (handed to us by the system via
//! `APP_CMD_INIT_WINDOW`) and owns the `WgpuRenderer` that draws into it.
//!
//! ## Lifecycle
//!
//! Android windows can be destroyed and recreated at any time (e.g. when the
//! app goes to the background and returns).  The window must handle:
//!
//! * `APP_CMD_INIT_WINDOW`  — create the wgpu surface and renderer.
//! * `APP_CMD_TERM_WINDOW`  — destroy the renderer, keep the window struct.
//! * `APP_CMD_WINDOW_RESIZED` — call `update_drawable_size`.
//!
//! ## Thread safety
//!
//! `AndroidWindow` is `Send + Sync`.  All renderer access is serialised through
//! a `Mutex<Option<WgpuRenderer>>`; the `Option` is `None` while the window
//! surface is unavailable.
//!
//! ## Input events
//!
//! Touch points and key events arrive via the NDK input queue and are
//! translated into the `TouchPoint` / `AndroidKeyEvent` types defined in
//! `mod.rs` before being forwarded to registered callbacks.
//!
//! ## GPUI integration
//!
//! `AndroidPlatformWindow` wraps an `Arc<AndroidWindow>` and implements
//! `gpui::PlatformWindow`, `HasWindowHandle`, and `HasDisplayHandle` so it
//! can be returned from `Platform::open_window`.

#![allow(unsafe_code)]

#[path = "window_activity.rs"]
mod window_activity;
use window_activity::WindowActivity;

use anyhow::{Context as _, Result};
use futures::channel::oneshot;
use gpui::{
    self, AtlasKey, AtlasTile, Capslock, DispatchEventResult, GpuSpecs, Modifiers, PlatformAtlas,
    PlatformDisplay, PlatformInputHandler, PlatformWindow, PromptButton, PromptLevel,
    RequestFrameOptions, WindowBackgroundAppearance, WindowBounds, WindowControlArea,
};
use gpui_wgpu::{wgpu, GpuContext, WgpuContext, WgpuRenderer, WgpuSurfaceConfig};
use parking_lot::Mutex;
use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, HasDisplayHandle, HasWindowHandle,
    RawDisplayHandle, RawWindowHandle,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use super::{AndroidKeyEvent, Bounds, DevicePixels, Pixels, Point, Size, TouchPoint};
use crate::mobile_events::{self, AppLifecycle, MobileEvent, MobileEventState, SurfaceLifecycle};
use crate::momentum::{MomentumScroller, VelocityTracker};

/// Lightweight, owned window handle for wgpu surface creation.
/// Stores the raw ANativeWindow pointer and implements the traits
/// required by `WgpuRenderer::new` (`Clone + Debug + Send + Sync + 'static`).
#[derive(Debug, Clone, Copy)]
struct RawAndroidWindow {
    nw_ptr: *mut std::ffi::c_void,
}

unsafe impl Send for RawAndroidWindow {}
unsafe impl Sync for RawAndroidWindow {}

impl HasWindowHandle for RawAndroidWindow {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
    {
        let ptr = NonNull::new(self.nw_ptr).ok_or(raw_window_handle::HandleError::Unavailable)?;
        let handle = AndroidNdkWindowHandle::new(ptr);
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(handle.into()) })
    }
}

impl HasDisplayHandle for RawAndroidWindow {
    fn display_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError>
    {
        Ok(unsafe {
            raw_window_handle::DisplayHandle::borrow_raw(RawDisplayHandle::Android(
                AndroidDisplayHandle::new(),
            ))
        })
    }
}

/// Shared momentum scrolling state, accessible from both the touch callback
/// (which starts/cancels flings and records velocity samples) and the
/// request-frame callback (which pumps the decelerating animation).
///
/// ## Coalesced scroll deltas
///
/// Android can deliver many `ACTION_MOVE` events between frames.  Instead of
/// dispatching a `ScrollWheel` event for every single move (which triggers a
/// full GPUI layout+paint each time), the touch callback **accumulates** the
/// delta into `pending_scroll_dx/dy`.  The `on_request_frame` callback then
/// drains the accumulated delta and emits a single `ScrollWheel` event per
/// frame.  This dramatically reduces the number of layout passes during a
/// drag and eliminates the "laggy" feeling on complex screens.
struct MomentumState {
    velocity_tracker: VelocityTracker,
    scroller: MomentumScroller,

    // ── Coalesced scroll state ───────────────────────────────────────────
    /// Accumulated scroll delta (logical px) from touch MOVE events since
    /// the last frame.  Drained by the frame callback.
    pending_scroll_dx: f32,
    pending_scroll_dy: f32,
    /// The most recent touch position (logical px) for the coalesced event.
    /// Updated on every MOVE so the ScrollWheel `position` field is correct.
    pending_scroll_pos_x: f32,
    pending_scroll_pos_y: f32,
    /// Whether there is a pending scroll delta to emit.
    has_pending_scroll: bool,
    /// The touch phase for the pending scroll event (Started for the first
    /// coalesced batch, Moved for subsequent ones).
    pending_scroll_phase: gpui::TouchPhase,
}

// Re-export for use with raw-window-handle and the frame-rate helper.
use ndk::native_window::NativeWindow;

type SetFrameRateFn = unsafe extern "C" fn(*mut ndk_sys::ANativeWindow, f32, i8) -> i32;
type SetFrameRateWithChangeStrategyFn =
    unsafe extern "C" fn(*mut ndk_sys::ANativeWindow, f32, i8, i8) -> i32;

const DEFAULT_FRAME_RATE_COMPATIBILITY: i8 = 0;
const ONLY_IF_SEAMLESS: i8 = 0;
const HIGH_FRAME_RATE: f32 = 120.0;

// Resolve an optional libandroid symbol at runtime.
//
// Linking API 30/31 frame-rate entry points directly would make the shared
// library fail to `dlopen` on older devices before our own code runs, so we
// always go through `dlsym`. Keep this dynamic lookup even if `ndk` grows
// nicer wrappers — reintroducing a static link would crash on pre-API-30.
fn load_libandroid_symbol(name: &[u8]) -> *mut c_void {
    debug_assert_eq!(name.last().copied(), Some(0));
    unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr().cast()) }
}

fn load_set_frame_rate() -> Option<SetFrameRateFn> {
    static SYMBOL: OnceLock<Option<SetFrameRateFn>> = OnceLock::new();
    *SYMBOL.get_or_init(|| {
        let symbol = load_libandroid_symbol(b"ANativeWindow_setFrameRate\0");
        if symbol.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute::<*mut c_void, SetFrameRateFn>(symbol) })
        }
    })
}

fn load_set_frame_rate_with_change_strategy() -> Option<SetFrameRateWithChangeStrategyFn> {
    static SYMBOL: OnceLock<Option<SetFrameRateWithChangeStrategyFn>> = OnceLock::new();
    *SYMBOL.get_or_init(|| {
        let symbol = load_libandroid_symbol(b"ANativeWindow_setFrameRateWithChangeStrategy\0");
        if symbol.is_null() {
            None
        } else {
            Some(unsafe {
                std::mem::transmute::<*mut c_void, SetFrameRateWithChangeStrategyFn>(symbol)
            })
        }
    })
}

/// Request 120 Hz refresh rate from the native window when the platform exposes the API.
///
/// The NDK entry points are resolved with `dlsym` so this library still loads
/// on pre-API-30/31 devices where the symbols do not exist.
fn request_high_frame_rate(window: &NativeWindow) {
    let api_level = unsafe { ndk_sys::android_get_device_api_level() };
    let native_window = window.ptr().as_ptr();

    let status = if api_level >= 31 {
        load_set_frame_rate_with_change_strategy().map(|set_frame_rate| unsafe {
            set_frame_rate(
                native_window,
                HIGH_FRAME_RATE,
                DEFAULT_FRAME_RATE_COMPATIBILITY,
                ONLY_IF_SEAMLESS,
            )
        })
    } else if api_level >= 30 {
        load_set_frame_rate().map(|set_frame_rate| unsafe {
            set_frame_rate(
                native_window,
                HIGH_FRAME_RATE,
                DEFAULT_FRAME_RATE_COMPATIBILITY,
            )
        })
    } else {
        None
    };

    match status {
        Some(0) => log::info!("Requested 120 Hz frame rate"),
        Some(status) => log::warn!("ANativeWindow_setFrameRate* failed with status {status}"),
        None => log::debug!("Skipping high frame rate request on API {api_level}"),
    }
}

// ── callback type aliases ─────────────────────────────────────────────────────

/// Called once per VSync tick to produce the next `Scene`.
pub type RequestFrameCallback = Box<dyn FnMut() + Send + 'static>;

/// Called when a touch event arrives.
pub type TouchCallback = Box<dyn FnMut(TouchPoint) + Send + 'static>;

/// Called when the window's active status changes (foreground/background).
pub type ActiveStatusCallback = Box<dyn FnMut(bool) + Send + 'static>;

/// Called when a key event arrives.
pub type KeyCallback = Box<dyn FnMut(AndroidKeyEvent) + Send + 'static>;

/// Called when the window is resized.
pub type ResizeCallback = Box<dyn FnMut(Size<DevicePixels>, f32) + Send + 'static>;

/// Called when the window is destroyed (surface lost).
pub type CloseCallback = Box<dyn FnOnce() + Send + 'static>;

/// Called when the window appearance (light/dark) changes.
pub type AppearanceCallback = Box<dyn FnMut(WindowAppearance) + Send + 'static>;

// ── appearance ────────────────────────────────────────────────────────────────

/// Light / dark / high-contrast appearance of the window.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum WindowAppearance {
    #[default]
    Light,
    Dark,
    HighContrastLight,
    HighContrastDark,
}

// ── window state (interior-mutable) ──────────────────────────────────────────

/// Safe area insets in physical (device) pixels.
///
/// These represent the areas of the screen occupied by system UI elements
/// (status bar, navigation bar, camera notch, etc.) that the app content
/// should avoid drawing into — or at least pad/account for.
#[derive(Debug, Clone, Copy, Default)]
pub struct SafeAreaInsets {
    /// Top inset in device pixels (status bar / camera notch).
    pub top: f32,
    /// Bottom inset in device pixels (navigation bar / gesture indicator).
    pub bottom: f32,
    /// Left inset in device pixels (e.g. display cutout on landscape).
    pub left: f32,
    /// Right inset in device pixels (e.g. display cutout on landscape).
    pub right: f32,
}

impl SafeAreaInsets {
    /// Convert physical-pixel insets to logical pixels using the given scale factor.
    pub fn to_logical(&self, scale_factor: f32) -> SafeAreaInsets {
        SafeAreaInsets {
            top: self.top / scale_factor,
            bottom: self.bottom / scale_factor,
            left: self.left / scale_factor,
            right: self.right / scale_factor,
        }
    }
}

struct WindowState {
    gpui_handle: Option<gpui::AnyWindowHandle>,
    key_meta_state: i32,
    /// The `NativeWindow` handle.  Reference-counted via `ndk::NativeWindow`
    /// (Clone acquires, Drop releases).  `None` while the surface is
    /// unavailable (between `term_window` and the next `init_window`).
    native_window: Option<NativeWindow>,

    /// Shared GPU context (instance + adapter + device + queue).
    /// Created once and reused across window re-creations.
    gpu_context: GpuContext,

    /// The wgpu renderer from `gpui_wgpu`.  `None` while the surface is unavailable.
    renderer: Option<WgpuRenderer>,

    /// Cached display geometry.
    width: i32,
    height: i32,
    scale_factor: f32,

    /// Safe area insets in physical (device) pixels.
    ///
    /// Updated from `AndroidApp::content_rect()` during `InitWindow` and
    /// `WindowResized`.  The top inset accounts for the status bar / notch,
    /// and the bottom inset accounts for the navigation bar / gesture area.
    safe_area_insets: SafeAreaInsets,
    has_native_insets: bool,

    /// Current appearance.
    appearance: WindowAppearance,

    /// Whether the window background should be transparent.
    transparent: bool,

    // ── callbacks ─────────────────────────────────────────────────────────
    request_frame_callback: Option<RequestFrameCallback>,
    touch_callback: Option<TouchCallback>,
    key_callback: Option<KeyCallback>,
    ime_callback: Option<Box<dyn FnMut(i64, i64, crate::mobile_events::ImeInput) + Send>>,
    ime_owner_refresh: Option<Box<dyn FnMut() + Send>>,
    resize_callback: Option<ResizeCallback>,
    close_callback: Option<CloseCallback>,
    appearance_callback: Option<AppearanceCallback>,
}

// SAFETY: `WindowState` is only ever accessed while holding the
// `Mutex<WindowState>` lock, and all GPU work (including any use of
// `GpuContext = Rc<RefCell<Option<WgpuContext>>>`) happens exclusively on the
// Android main thread.  The `Rc` never escapes to another thread.
unsafe impl Send for WindowState {}

// ── AndroidWindow ─────────────────────────────────────────────────────────────

/// A GPUI window on Android.
///
/// Each `AndroidWindow` corresponds to one `ANativeWindow *` provided by the
/// system.  On a typical Android device there is exactly one window per app,
/// but foldable / multi-display devices may have two.
pub struct AndroidWindow {
    state: Arc<Mutex<WindowState>>,
    /// A stable numeric ID derived from the initial native-window pointer.
    id: u64,
    activity: WindowActivity,
    mobile_events: Mutex<MobileEventState>,
    legacy_multitouch: std::sync::atomic::AtomicBool,
    app_resumed: std::sync::atomic::AtomicBool,
    visibility_callback: Mutex<Option<Box<dyn FnMut(gpui::WindowVisibility) + Send>>>,
}

// SAFETY: `WindowState` is protected by a `Mutex`.
unsafe impl Send for AndroidWindow {}
unsafe impl Sync for AndroidWindow {}

impl AndroidWindow {
    pub(crate) fn set_gpui_handle(&self, handle: gpui::AnyWindowHandle) {
        self.state.lock().gpui_handle = Some(handle);
    }

    pub(crate) fn gpui_handle(&self) -> Option<gpui::AnyWindowHandle> {
        self.state.lock().gpui_handle
    }
    fn emit_mobile_events(&self, events: Vec<MobileEvent>) {
        mobile_events::dispatch_batch(self.id, events);
    }

    /// Activity resume/pause, distinct from focus and system sleep. Call on the UI thread.
    pub fn notify_app_lifecycle(&self, resumed: bool) {
        use std::sync::atomic::Ordering;
        let before = self.visibility();
        self.app_resumed.store(resumed, Ordering::Relaxed);
        let lifecycle = if resumed {
            AppLifecycle::Resumed
        } else {
            AppLifecycle::Paused
        };
        let events = self
            .mobile_events
            .lock()
            .lifecycle(lifecycle, std::time::Duration::ZERO);
        if !resumed {
            self.cancel_legacy_touch();
            self.legacy_multitouch.store(false, Ordering::Relaxed);
        }
        self.emit_mobile_events(events);
        self.notify_visibility(before);
    }

    fn visibility(&self) -> gpui::WindowVisibility {
        use std::sync::atomic::Ordering;
        if self.app_resumed.load(Ordering::Relaxed) && self.activity.has_surface() {
            gpui::WindowVisibility::Visible
        } else {
            gpui::WindowVisibility::Hidden
        }
    }

    fn notify_visibility(&self, previous: gpui::WindowVisibility) {
        let current = self.visibility();
        if previous != current {
            let callback = self.visibility_callback.lock().take();
            if let Some(mut callback) = callback {
                callback(current);
                let mut slot = self.visibility_callback.lock();
                if slot.is_none() {
                    *slot = Some(callback);
                }
            }
        }
    }

    fn cancel_legacy_touch(&self) {
        let callback = self.state.lock().touch_callback.take();
        if let Some(mut callback) = callback {
            callback(TouchPoint {
                id: 0,
                x: 0.0,
                y: 0.0,
                action: 3,
            });
            let mut state = self.state.lock();
            if state.touch_callback.is_none() {
                state.touch_callback = Some(callback);
            }
        }
    }

    // ── constructors ─────────────────────────────────────────────────────────

    /// Create an `AndroidWindow` from an `ndk::NativeWindow`.
    ///
    /// Initialises the wgpu surface and renderer immediately.
    ///
    /// `gpu_context` — shared wgpu device/queue/instance.  Pass `None` on the
    /// first window; it will be initialised and stored.  Subsequent windows
    /// should pass the same `Option<WgpuContext>` so that the GPU context is
    /// shared.
    ///
    /// `scale_factor` — the display density relative to 160 dpi (e.g. 3.0 for
    /// a 480 dpi device).
    ///
    /// `transparent` — whether to request a pre-multiplied alpha surface.
    pub fn new(
        native_window: NativeWindow,
        gpu_context: GpuContext,
        scale_factor: f32,
        transparent: bool,
    ) -> Result<Arc<Self>> {
        request_high_frame_rate(&native_window);

        let width = native_window.width();
        let height = native_window.height();

        log::info!(
            "AndroidWindow::new — {}×{} scale={:.1}",
            width,
            height,
            scale_factor
        );

        let renderer = Self::create_renderer(
            &native_window,
            Rc::clone(&gpu_context),
            width,
            height,
            transparent,
        )
        .context("failed to create gpui_wgpu renderer")?;

        let id = native_window.ptr().as_ptr() as u64;

        let state = Arc::new(Mutex::new(WindowState {
            gpui_handle: None,
            key_meta_state: 0,
            native_window: Some(native_window),
            gpu_context,
            renderer: Some(renderer),
            width,
            height,
            scale_factor,
            safe_area_insets: SafeAreaInsets::default(),
            has_native_insets: false,
            appearance: WindowAppearance::Light,
            transparent,
            request_frame_callback: None,
            touch_callback: None,
            key_callback: None,
            ime_callback: None,
            ime_owner_refresh: None,
            resize_callback: None,
            close_callback: None,
            appearance_callback: None,
        }));

        Ok(Arc::new(Self {
            state,
            id,
            activity: WindowActivity::new(false, true),
            mobile_events: Mutex::new({
                let mut events = MobileEventState::default();
                events.surface(SurfaceLifecycle::Available, std::time::Duration::ZERO);
                events
            }),
            legacy_multitouch: std::sync::atomic::AtomicBool::new(false),
            app_resumed: std::sync::atomic::AtomicBool::new(true),
            visibility_callback: Mutex::new(None),
        }))
    }

    /// Create a headless `AndroidWindow` for testing.
    ///
    /// No wgpu surface is created.
    pub fn headless(width: i32, height: i32, scale_factor: f32) -> Arc<Self> {
        let state = Arc::new(Mutex::new(WindowState {
            gpui_handle: None,
            key_meta_state: 0,
            native_window: None,
            gpu_context: Rc::new(RefCell::new(None)),
            renderer: None,
            width,
            height,
            scale_factor,
            safe_area_insets: SafeAreaInsets::default(),
            has_native_insets: false,
            appearance: WindowAppearance::Light,
            transparent: false,
            request_frame_callback: None,
            touch_callback: None,
            key_callback: None,
            ime_callback: None,
            ime_owner_refresh: None,
            resize_callback: None,
            close_callback: None,
            appearance_callback: None,
        }));

        Arc::new(Self {
            state,
            id: ((width as u64) << 32) | (height as u64),
            activity: WindowActivity::new(false, false),
            mobile_events: Mutex::new(MobileEventState::default()),
            legacy_multitouch: std::sync::atomic::AtomicBool::new(false),
            app_resumed: std::sync::atomic::AtomicBool::new(false),
            visibility_callback: Mutex::new(None),
        })
    }

    // ── window lifecycle ──────────────────────────────────────────────────────

    /// Called when `APP_CMD_INIT_WINDOW` fires and a new `NativeWindow` is
    /// available (e.g. after returning from the background).
    pub fn init_window(&self, native_window: NativeWindow, gpu_context: GpuContext) -> Result<()> {
        let previous_visibility = self.visibility();
        request_high_frame_rate(&native_window);

        let width = native_window.width();
        let height = native_window.height();

        let mut state = self.state.lock();
        let transparent = state.transparent;

        // If a renderer already exists (kept alive across term_window), just
        // replace its surface.  This preserves the atlas and all cached
        // AtlasTextureIds so GPUI's scene cache remains valid.
        if state.renderer.is_some() {
            let raw = Self::raw_window(&native_window);
            let config = WgpuSurfaceConfig {
                size: gpui::size(gpui::DevicePixels(width), gpui::DevicePixels(height)),
                transparent,
                preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
            };
            let instance = state
                .gpu_context
                .borrow()
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("gpu_context missing during surface replacement"))?
                .instance
                .clone();
            state
                .renderer
                .as_mut()
                .unwrap()
                .replace_surface(&raw, config, &instance)?;
            log::info!(
                "AndroidWindow::init_window — replaced surface {}×{}",
                width,
                height
            );
        } else {
            // First init or after a full destroy — create a fresh renderer.
            let ctx = if state.gpu_context.borrow().is_some() {
                Rc::clone(&state.gpu_context)
            } else {
                // Use the context from the platform; store it in our state too.
                state.gpu_context = Rc::clone(&gpu_context);
                gpu_context
            };
            let renderer = Self::create_renderer(&native_window, ctx, width, height, transparent)?;
            state.renderer = Some(renderer);
            log::info!(
                "AndroidWindow::init_window — created new renderer {}×{}",
                width,
                height
            );
        }

        // Store the new native window (drops previous one if any).
        state.native_window = Some(native_window);
        state.width = width;
        state.height = height;
        drop(state);
        self.activity.set_surface(true);
        let events = self
            .mobile_events
            .lock()
            .surface(SurfaceLifecycle::Available, std::time::Duration::ZERO);
        self.emit_mobile_events(events);
        self.notify_visibility(previous_visibility);
        Ok(())
    }

    /// Called when `APP_CMD_TERM_WINDOW` fires and the surface is about to be
    /// destroyed.
    ///
    /// Drops the renderer (and therefore the wgpu surface) but keeps the window
    /// struct alive so callbacks are preserved.
    pub fn term_window(&self) {
        self.legacy_multitouch
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let previous_visibility = self.visibility();
        self.activity.set_surface(false);
        let events = self
            .mobile_events
            .lock()
            .surface(SurfaceLifecycle::Lost, std::time::Duration::ZERO);
        self.cancel_legacy_touch();
        self.emit_mobile_events(events);
        self.notify_visibility(previous_visibility);
        let mut state = self.state.lock();

        // Unconfigure the surface so the renderer stops trying to present,
        // but keep the renderer alive so the atlas (with all cached
        // texture IDs) survives across the background/foreground cycle.
        if let Some(ref mut renderer) = state.renderer {
            renderer.unconfigure_surface();
            log::info!("AndroidWindow::term_window — surface unconfigured (renderer kept alive)");
        }

        // Release our reference on the native window.
        state.native_window = None;

        // NOTE: We intentionally do NOT fire the close callback here.
        // On Android, term_window means the surface is being destroyed
        // (e.g. the app is going to the background), but the logical
        // window and all its GPUI callbacks should stay alive so that
        // when the surface is recreated on resume, rendering resumes
        // seamlessly via init_window().
    }

    /// Called when `APP_CMD_WINDOW_RESIZED` fires.
    pub fn handle_resize(&self) {
        let (new_w, new_h, scale) = {
            let mut state = self.state.lock();

            let nw = match state.native_window.as_ref() {
                Some(nw) => nw,
                None => return,
            };

            let new_w = nw.width();
            let new_h = nw.height();

            if new_w == state.width && new_h == state.height {
                log::debug!("handle_resize: no change ({}×{})", new_w, new_h);
                return;
            }

            log::info!(
                "AndroidWindow resize: {}×{} → {}×{}",
                state.width,
                state.height,
                new_w,
                new_h
            );

            state.width = new_w;
            state.height = new_h;
            let scale = state.scale_factor;

            // update_drawable_size calls device.poll(Wait) which can take
            // time — take the renderer out to avoid holding the state lock.
            if let Some(mut renderer) = state.renderer.take() {
                renderer.update_drawable_size(gpui::size(
                    gpui::DevicePixels(new_w),
                    gpui::DevicePixels(new_h),
                ));
                state.renderer = Some(renderer);
            }

            (new_w, new_h, scale)
        }; // state lock dropped

        // Fire the resize callback outside the lock — GPUI's callback may
        // call bounds() / scale_factor() which need the state lock.
        let cb = {
            let mut state = self.state.lock();
            state.resize_callback.take()
        };
        if let Some(mut cb) = cb {
            cb(
                Size {
                    width: DevicePixels(new_w),
                    height: DevicePixels(new_h),
                },
                scale,
            );
            // Put callback back.
            let mut state = self.state.lock();
            if state.resize_callback.is_none() {
                state.resize_callback = Some(cb);
            }
        }
    }

    // ── drawing ───────────────────────────────────────────────────────────────

    /// Returns `true` when the scene contains no renderable primitives.
    ///
    /// Checks every primitive bucket instead of just `quads`, because text
    /// and images are emitted as sprite primitives.
    fn scene_is_empty(scene: &gpui::Scene) -> bool {
        scene.shadows.is_empty()
            && scene.quads.is_empty()
            && scene.paths.is_empty()
            && scene.underlines.is_empty()
            && scene.monochrome_sprites.is_empty()
            && scene.subpixel_sprites.is_empty()
            && scene.polychrome_sprites.is_empty()
            && scene.surfaces.is_empty()
    }

    /// Draw `scene` into the window's next frame.
    ///
    /// Accepts a `gpui::Scene` directly — the `gpui_wgpu::WgpuRenderer`
    /// natively consumes it without any type bridging.
    ///
    /// No-ops if the renderer is not available (surface lost).
    ///
    /// **Guard against empty scenes**: If the scene contains zero
    /// primitives (no quads, shadows, paths, underlines, or sprites)
    /// we skip the draw entirely.  The upstream `gpui_wgpu` renderer
    /// clears the surface to transparent/black before drawing, so
    /// presenting an empty scene produces a visible flash where all
    /// content disappears for one frame.  This commonly happens:
    ///
    /// - During the first few event-loop iterations before GPUI has
    ///   finished building the view tree.
    /// - When the surface is reconfigured after a Lost/Outdated error
    ///   and the next frame callback produces an empty scene.
    /// - Intermittently during fast scrolling if the layout pass
    ///   hasn't produced new content yet.
    pub fn draw(&self, scene: &gpui::Scene) {
        if !self.visibility().is_visible() {
            return;
        }
        // Skip only truly empty scenes. Text and images are emitted as sprite
        // primitives, not quads, so checking `scene.quads` alone can drop
        // valid text-only frames during scroll or cache refresh and produce
        // the flicker we were trying to avoid.
        if Self::scene_is_empty(scene) {
            log::trace!("AndroidWindow::draw — skipping empty scene");
            return;
        }

        // Take the renderer out of state so we can draw WITHOUT holding
        // the state lock.  renderer.draw() calls get_current_texture()
        // which can block on the GPU / Vulkan driver.  Holding the lock
        // during that time prevents all other state accessors (bounds,
        // scale_factor, etc.) from running, leading to deadlock when
        // GPUI's layout or the event loop needs state during a render.
        let mut renderer = {
            let mut state = self.state.lock();
            match state.renderer.take() {
                Some(r) => r,
                None => return,
            }
        };

        renderer.draw(scene);

        // Put the renderer back.
        let mut state = self.state.lock();
        state.renderer = Some(renderer);
    }

    /// Invoke the `request_frame_callback` if one is registered.
    ///
    /// Called by the event loop on every iteration (~60 fps).
    ///
    /// **Important**: The callback is taken out of the lock before being
    /// invoked and put back afterwards.  This avoids a deadlock: the GPUI
    /// callback runs layout → paint → `PlatformWindow::draw` →
    /// `AndroidWindow::draw`, which needs to acquire the same `state` lock
    /// to access the renderer.
    pub fn request_frame(&self) {
        if !self.visibility().is_visible() {
            return;
        }
        // Take the callback out of the lock so it can be invoked without
        // holding it.  This lets `draw()` (called from inside the callback)
        // acquire the lock for the renderer.
        let cb = {
            let mut state = self.state.lock();
            state.request_frame_callback.take()
        };

        if let Some(mut cb) = cb {
            cb();

            // Put the callback back so it fires again next frame.
            let mut state = self.state.lock();
            // Only put it back if nothing else registered a new callback
            // while we were running (unlikely but defensive).
            if state.request_frame_callback.is_none() {
                state.request_frame_callback = Some(cb);
            }
        }
    }

    // ── input event delivery ──────────────────────────────────────────────────

    /// Deliver a touch point to the registered touch callback.
    ///
    /// The callback is taken out of the lock before invocation (same pattern
    /// as `request_frame`) to avoid potential deadlocks if the callback
    /// re-enters any window method that needs the lock.
    pub fn handle_touch(&self, mut point: TouchPoint) {
        use std::sync::atomic::Ordering;
        let phase = match point.action {
            0 => gpui::TouchPhase::Started,
            1 => gpui::TouchPhase::Ended,
            2 => gpui::TouchPhase::Moved,
            3 => gpui::TouchPhase::Cancelled,
            _ => return,
        };
        let scale = self.scale_factor();
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let position = gpui::point(gpui::px(point.x / scale), gpui::px(point.y / scale));
        let (events, count) = {
            let mut state = self.mobile_events.lock();
            let mut events: Vec<MobileEvent> = if point.action == 3 {
                state.cancel_all(std::time::Duration::ZERO)
            } else {
                state
                    .touch(point.id as u64, phase, position, std::time::Duration::ZERO)
                    .into_iter()
                    .collect()
            };
            if !events.is_empty() && point.action != 3 {
                if let Some(pinch) = state.pointer_span(std::time::Duration::ZERO) {
                    events.push(pinch);
                }
            }
            (events, state.active_pointer_count())
        };
        let has_events = !events.is_empty();
        if let Some(MobileEvent::Touch { event, .. }) = events.first() {
            point.x = f32::from(event.position.x) * scale;
            point.y = f32::from(event.position.y) * scale;
        }
        self.emit_mobile_events(events);
        if count > 1 {
            self.legacy_multitouch.store(true, Ordering::Relaxed);
        }
        if crate::mobile_events::raw_touch_only() {
            self.cancel_legacy_touch();
            if count == 0 {
                self.legacy_multitouch.store(false, Ordering::Relaxed);
            }
            return;
        }
        if point.action == 3 || self.legacy_multitouch.load(Ordering::Relaxed) {
            self.cancel_legacy_touch();
            if count == 0 {
                self.legacy_multitouch.store(false, Ordering::Relaxed);
            }
            return;
        }
        if !has_events {
            return;
        }
        let cb = {
            let mut state = self.state.lock();
            state.touch_callback.take()
        };
        if let Some(mut cb) = cb {
            let scale = self.scale_factor();
            log::debug!(
                "handle_touch: id={} action={} phys=({:.0},{:.0}) logical=({:.0},{:.0}) scale={:.1}",
                point.id,
                point.action,
                point.x,
                point.y,
                point.x / scale,
                point.y / scale,
                scale,
            );
            cb(point);
            let mut state = self.state.lock();
            if state.touch_callback.is_none() {
                state.touch_callback = Some(cb);
            }
        } else {
            log::warn!(
                "handle_touch: NO touch_callback registered — touch dropped (id={} action={})",
                point.id,
                point.action,
            );
        }
    }

    /// Deliver a key event to the registered key callback.
    pub fn handle_key_event(&self, event: AndroidKeyEvent) {
        let cb = {
            let mut state = self.state.lock();
            state.key_meta_state = event.meta_state;
            state.key_callback.take()
        };
        if let Some(mut cb) = cb {
            cb(event);
            let mut state = self.state.lock();
            if state.key_callback.is_none() {
                state.key_callback = Some(cb);
            }
        }
    }

    pub(crate) fn handle_ime_input(
        &self,
        session: i64,
        editor: i64,
        input: crate::mobile_events::ImeInput,
    ) {
        let callback = self.state.lock().ime_callback.take();
        if let Some(mut callback) = callback {
            callback(session, editor, input);
            self.state.lock().ime_callback.get_or_insert(callback);
        }
    }

    pub(crate) fn refresh_ime_owner(&self) {
        self.activity.flush();
        let callback = self.state.lock().ime_owner_refresh.take();
        if let Some(mut callback) = callback {
            callback();
            self.state.lock().ime_owner_refresh.get_or_insert(callback);
        }
    }

    // ── appearance ────────────────────────────────────────────────────────────

    /// Update the window's appearance (e.g. after a dark-mode change).
    pub fn set_appearance(&self, appearance: WindowAppearance) {
        let changed = {
            let mut state = self.state.lock();
            if state.appearance == appearance {
                return;
            }
            state.appearance = appearance;
            true
        };
        if changed {
            // Fire callback outside the lock — it may call back into
            // GPUI which needs state access.
            let cb = {
                let mut state = self.state.lock();
                state.appearance_callback.take()
            };
            if let Some(mut cb) = cb {
                cb(appearance);
                let mut state = self.state.lock();
                if state.appearance_callback.is_none() {
                    state.appearance_callback = Some(cb);
                }
            }
        }
    }

    /// Returns the current appearance.
    pub fn appearance(&self) -> WindowAppearance {
        self.state.lock().appearance
    }

    /// Deliver OS window focus outside poll_events and the renderer state lock.
    pub fn set_active(&self, active: bool) {
        if !active {
            self.state.lock().key_meta_state = 0;
        }
        self.activity.set_active(active);
    }

    // ── transparency ──────────────────────────────────────────────────────────

    /// Enable or disable transparent compositing.
    pub fn set_transparent(&self, transparent: bool) {
        let mut state = self.state.lock();
        if state.transparent == transparent {
            return;
        }
        state.transparent = transparent;
        if let Some(renderer) = state.renderer.as_mut() {
            renderer.update_transparency(transparent);
        }
    }

    // ── geometry / scale ──────────────────────────────────────────────────────

    /// Physical size of the window in device pixels.
    pub fn physical_size(&self) -> Size<DevicePixels> {
        let state = self.state.lock();
        Size {
            width: DevicePixels(state.width),
            height: DevicePixels(state.height),
        }
    }

    /// Logical size of the window in density-independent pixels.
    pub fn logical_size(&self) -> Size<Pixels> {
        let state = self.state.lock();
        Size {
            width: Pixels(state.width as f32 / state.scale_factor),
            height: Pixels(state.height as f32 / state.scale_factor),
        }
    }

    /// Physical bounds with origin at `(0, 0)`.
    pub fn bounds(&self) -> Bounds<DevicePixels> {
        Bounds {
            origin: Point {
                x: DevicePixels(0),
                y: DevicePixels(0),
            },
            size: self.physical_size(),
        }
    }

    /// Display scale factor (device pixels per logical pixel).
    pub fn scale_factor(&self) -> f32 {
        self.state.lock().scale_factor
    }

    /// Returns the current safe area insets in physical (device) pixels.
    pub fn safe_area_insets(&self) -> SafeAreaInsets {
        self.state.lock().safe_area_insets
    }

    /// Returns the current safe area insets in logical pixels.
    pub fn safe_area_insets_logical(&self) -> SafeAreaInsets {
        let state = self.state.lock();
        state.safe_area_insets.to_logical(state.scale_factor)
    }

    /// Update the safe area insets from the content rect provided by the system.
    ///
    /// `content_rect` is `(left, top, right, bottom)` in physical pixels — the
    /// area of the window NOT covered by system bars.  We compute insets by
    /// subtracting from the full window dimensions.
    pub fn update_safe_area_from_content_rect(
        &self,
        content_left: i32,
        content_top: i32,
        content_right: i32,
        content_bottom: i32,
    ) {
        let mut state = self.state.lock();
        if state.has_native_insets {
            return;
        }
        let insets = SafeAreaInsets {
            top: content_top as f32,
            bottom: (state.height - content_bottom).max(0) as f32,
            left: content_left as f32,
            right: (state.width - content_right).max(0) as f32,
        };
        log::info!(
            "safe_area_insets updated: top={:.0} bottom={:.0} left={:.0} right={:.0} (physical px)",
            insets.top,
            insets.bottom,
            insets.left,
            insets.right,
        );
        state.safe_area_insets = insets;
        let logical = insets.to_logical(state.scale_factor);
        drop(state);
        mobile_events::dispatch(
            self.id,
            &MobileEvent::Insets(crate::mobile_events::WindowInsets {
                safe_area: gpui::Edges {
                    top: gpui::px(logical.top),
                    bottom: gpui::px(logical.bottom),
                    left: gpui::px(logical.left),
                    right: gpui::px(logical.right),
                },
                keyboard: None,
                keyboard_visible: None,
                keyboard_insets: None,
            }),
        );
    }

    /// Called on the GPUI thread after the Java UI thread reports typed WindowInsets.
    pub(crate) fn update_window_insets(
        &self,
        density: f32,
        insets: crate::mobile_events::WindowInsets,
    ) {
        let mut state = self.state.lock();
        state.has_native_insets = true;
        state.safe_area_insets = SafeAreaInsets {
            top: f32::from(insets.safe_area.top) * density,
            bottom: f32::from(insets.safe_area.bottom) * density,
            left: f32::from(insets.safe_area.left) * density,
            right: f32::from(insets.safe_area.right) * density,
        };
        let density_changed = state.scale_factor != density;
        state.scale_factor = density;
        let dimensions = Size {
            width: DevicePixels(state.width),
            height: DevicePixels(state.height),
        };
        let callback = if density_changed {
            state.resize_callback.take()
        } else {
            None
        };
        drop(state);
        crate::set_keyboard_height(
            insets
                .keyboard_insets
                .as_ref()
                .map(|edges| f32::from(edges.bottom))
                .unwrap_or(0.0),
        );
        mobile_events::dispatch(self.id, &MobileEvent::Insets(insets));
        if let Some(mut callback) = callback {
            callback(dimensions, density);
            let mut state = self.state.lock();
            if state.resize_callback.is_none() {
                state.resize_callback = Some(callback);
            }
        }
    }

    // ── state queries ─────────────────────────────────────────────────────────

    /// Whether the window currently has an active wgpu surface.
    pub fn has_surface(&self) -> bool {
        self.state.lock().renderer.is_some()
    }

    /// Returns the sprite atlas from the renderer, if available.
    pub fn sprite_atlas(&self) -> Option<Arc<dyn PlatformAtlas>> {
        let state = self.state.lock();
        state
            .renderer
            .as_ref()
            .map(|r| r.sprite_atlas().clone() as Arc<dyn PlatformAtlas>)
    }

    /// Returns GPU specs from the renderer, if available.
    pub fn gpu_specs(&self) -> Option<GpuSpecs> {
        let state = self.state.lock();
        state.renderer.as_ref().and_then(|r| r.gpu_specs())
    }

    /// Whether the window is currently active / visible.
    pub fn is_active(&self) -> bool {
        self.activity.is_active()
    }

    /// A stable numeric identifier for this window.
    pub fn id(&self) -> u64 {
        self.id
    }

    // ── callback registration ─────────────────────────────────────────────────

    /// Register a callback invoked once per VSync tick.
    pub fn on_request_frame<F>(&self, cb: F)
    where
        F: FnMut() + Send + 'static,
    {
        self.state.lock().request_frame_callback = Some(Box::new(cb));
    }

    /// Register a callback invoked for each touch point.
    pub fn on_touch<F>(&self, cb: F)
    where
        F: FnMut(TouchPoint) + Send + 'static,
    {
        self.state.lock().touch_callback = Some(Box::new(cb));
    }

    /// Register a callback invoked for each key event.
    pub fn on_key_event<F>(&self, cb: F)
    where
        F: FnMut(AndroidKeyEvent) + Send + 'static,
    {
        self.state.lock().key_callback = Some(Box::new(cb));
    }

    /// Register a callback invoked when the window is resized.
    pub fn on_resize<F>(&self, cb: F)
    where
        F: FnMut(Size<DevicePixels>, f32) + Send + 'static,
    {
        self.state.lock().resize_callback = Some(Box::new(cb));
    }

    /// Register a callback invoked once when the window surface is lost.
    pub fn on_close<F>(&self, cb: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.state.lock().close_callback = Some(Box::new(cb));
    }

    /// Register a callback invoked when the appearance (light/dark) changes.
    pub fn on_appearance_changed<F>(&self, cb: F)
    where
        F: FnMut(WindowAppearance) + Send + 'static,
    {
        self.state.lock().appearance_callback = Some(Box::new(cb));
    }

    /// Register a callback invoked when the window's active status changes.
    pub fn on_active_status_change<F>(&self, cb: F)
    where
        F: FnMut(bool) + Send + 'static,
    {
        self.activity.on_change(Box::new(cb));
    }

    // ── GPU introspection ─────────────────────────────────────────────────────

    /// Whether the GPU supports dual-source blending (subpixel text AA).
    pub fn supports_subpixel_aa(&self) -> bool {
        self.state
            .lock()
            .renderer
            .as_ref()
            .map(|r| r.supports_dual_source_blending())
            .unwrap_or(false)
    }

    // ── private helpers ───────────────────────────────────────────────────────

    /// Build a raw-window-handle wrapper for the given `NativeWindow`.
    ///
    /// `ndk::NativeWindow` implements `HasWindowHandle` but not `HasDisplayHandle`,
    /// so we wrap both into a single owned struct that satisfies the
    /// `Clone + Debug + Send + Sync + 'static` bounds required by `WgpuRenderer::new`.
    fn raw_window(native_window: &NativeWindow) -> RawAndroidWindow {
        let ptr = native_window
            .window_handle()
            .expect("NativeWindow handle unavailable")
            .as_raw();
        // Extract the raw pointer from the RawWindowHandle
        let nw_ptr = match ptr {
            RawWindowHandle::AndroidNdk(h) => h.a_native_window.as_ptr(),
            _ => panic!("Expected AndroidNdk window handle"),
        };
        RawAndroidWindow { nw_ptr }
    }

    /// Create a `WgpuRenderer` for the given `NativeWindow`.
    fn create_renderer(
        native_window: &NativeWindow,
        gpu_context: GpuContext,
        width: i32,
        height: i32,
        transparent: bool,
    ) -> Result<WgpuRenderer> {
        let raw = Self::raw_window(native_window);

        let mut qemu = [0 as libc::c_char; libc::PROP_VALUE_MAX as usize];
        // Android's property API writes at most PROP_VALUE_MAX bytes, including NUL.
        let is_emulator = unsafe {
            libc::__system_property_get(c"ro.kernel.qemu".as_ptr(), qemu.as_mut_ptr()) == 1
                && qemu[0] == b'1' as libc::c_char
        };
        if is_emulator && gpu_context.borrow().is_none() {
            // ranchu can crash in vkSetDebugUtilsObjectNameEXT, even with SwiftShader.
            // Keep Vulkan/debugging/validation; omit only HAL object labels.
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
                flags: wgpu::InstanceFlags::default() | wgpu::InstanceFlags::DISCARD_HAL_LABELS,
                backend_options: wgpu::BackendOptions::default(),
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
                display: Some(Box::new(raw)),
            });
            // The native window stays alive throughout adapter selection. This temporary
            // surface is dropped before the renderer creates its surface on the same instance.
            let surface = unsafe {
                instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: None,
                    raw_window_handle: raw.window_handle()?.as_raw(),
                })?
            };
            let context = WgpuContext::new(instance, &surface, None)?;
            *gpu_context.borrow_mut() = Some(context);
            log::info!("Android emulator Vulkan workaround: HAL object labels disabled");
        }

        let config = WgpuSurfaceConfig {
            size: gpui::size(gpui::DevicePixels(width), gpui::DevicePixels(height)),
            transparent,
            preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
        };

        WgpuRenderer::new(gpu_context, &raw, config, None)
    }
}

impl Drop for AndroidWindow {
    fn drop(&mut self) {
        let mut state = self.state.lock();

        // Destroy renderer before releasing the native window.
        if let Some(mut renderer) = state.renderer.take() {
            renderer.destroy();
        }

        state.native_window = None;
    }
}

// ── AndroidPlatformWindow (PlatformWindow impl) ──────────────────────────────

/// A wrapper around `Arc<AndroidWindow>` that implements `gpui::PlatformWindow`.
///
/// GPUI expects `Box<dyn PlatformWindow>` from `Platform::open_window`.  This
/// struct provides the trait implementation by delegating to the underlying
/// `AndroidWindow` methods.
#[allow(clippy::type_complexity)]
pub struct AndroidPlatformWindow {
    window: Arc<AndroidWindow>,
    display: Option<Rc<dyn PlatformDisplay>>,
    input_handler: Rc<crate::mobile_dispatch::Callback<PlatformInputHandler>>,
    title: String,
    /// Shared momentum scrolling state — used by both the touch callback
    /// (to start/cancel flings) and the frame callback (to pump inertia).
    momentum: Arc<Mutex<MomentumState>>,
    /// Shared reference to the GPUI input callback, so the frame callback can
    /// emit synthetic momentum ScrollWheel events.  Initialised to a no-op;
    /// replaced when `on_input` is called.
    momentum_input_cb:
        Arc<Mutex<Box<dyn FnMut(gpui::PlatformInput) -> DispatchEventResult + Send>>>,
}

impl AndroidPlatformWindow {
    /// Create a new `AndroidPlatformWindow` wrapping an existing `AndroidWindow`.
    pub fn new(window: Arc<AndroidWindow>, display: Option<Rc<dyn PlatformDisplay>>) -> Self {
        // No-op input callback used until on_input is called.
        let noop_input_cb: Box<dyn FnMut(gpui::PlatformInput) -> DispatchEventResult + Send> =
            Box::new(|_| DispatchEventResult::default());
        Self {
            window,
            display,
            input_handler: Rc::new(crate::mobile_dispatch::Callback::default()),
            title: String::new(),
            momentum: Arc::new(Mutex::new(MomentumState {
                velocity_tracker: VelocityTracker::new(),
                scroller: MomentumScroller::new(),
                pending_scroll_dx: 0.0,
                pending_scroll_dy: 0.0,
                pending_scroll_pos_x: 0.0,
                pending_scroll_pos_y: 0.0,
                has_pending_scroll: false,
                pending_scroll_phase: gpui::TouchPhase::Moved,
            })),
            momentum_input_cb: Arc::new(Mutex::new(noop_input_cb)),
        }
    }

    /// Access the underlying `AndroidWindow`.
    pub fn inner(&self) -> &Arc<AndroidWindow> {
        &self.window
    }
}

impl Drop for AndroidPlatformWindow {
    fn drop(&mut self) {
        let owns_ime = crate::mobile_events::active_text_input_lease().is_none_or(|lease| {
            self.window
                .gpui_handle()
                .is_some_and(|handle| handle.window_id().as_u64() == lease.window())
        });
        if let Some(lease) = crate::mobile_events::active_text_input_lease() {
            if self
                .window
                .gpui_handle()
                .is_some_and(|handle| handle.window_id().as_u64() == lease.window())
            {
                crate::mobile_events::release_text_input_owner(lease);
            }
        }
        self.input_handler.set(None);
        if owns_ime {
            super::jni::IME_EDITOR.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            super::jni::IME_HAS_HANDLER.store(false, std::sync::atomic::Ordering::Release);
            super::jni::hide_keyboard_android();
        }
    }
}

impl HasWindowHandle for AndroidPlatformWindow {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
    {
        let state = self.window.state.lock();
        let nw = state
            .native_window
            .as_ref()
            .ok_or(raw_window_handle::HandleError::Unavailable)?;
        // Build the handle from the raw pointer.  The pointer remains valid
        // because AndroidWindow holds a NativeWindow reference for as long
        // as this AndroidPlatformWindow (and thus `self`) is alive.
        let handle = AndroidNdkWindowHandle::new(NonNull::new(nw.ptr().as_ptr().cast()).unwrap());
        let raw = RawWindowHandle::AndroidNdk(handle);
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for AndroidPlatformWindow {
    fn display_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError>
    {
        let raw = RawDisplayHandle::Android(AndroidDisplayHandle::new());
        // SAFETY: Android display handle has no lifetime requirements.
        Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(raw) })
    }
}

impl PlatformWindow for AndroidPlatformWindow {
    fn bounds(&self) -> gpui::Bounds<gpui::Pixels> {
        let state = self.window.state.lock();
        let w = state.width as f32 / state.scale_factor;
        let h = state.height as f32 / state.scale_factor;
        gpui::Bounds {
            origin: gpui::point(gpui::px(0.0), gpui::px(0.0)),
            size: gpui::size(gpui::px(w), gpui::px(h)),
        }
    }

    fn is_maximized(&self) -> bool {
        // Android windows are always effectively maximized (fullscreen).
        true
    }

    fn window_bounds(&self) -> WindowBounds {
        // Android windows are always fullscreen.
        WindowBounds::Fullscreen(self.bounds())
    }

    fn content_size(&self) -> gpui::Size<gpui::Pixels> {
        self.bounds().size
    }

    fn resize(&mut self, _size: gpui::Size<gpui::Pixels>) {
        // Android windows are resized by the system, not the application.
        // No-op: the system controls window size.
    }

    fn scale_factor(&self) -> f32 {
        self.window.scale_factor()
    }

    fn appearance(&self) -> gpui::WindowAppearance {
        let local_appearance = self.window.appearance();
        match local_appearance {
            WindowAppearance::Dark => gpui::WindowAppearance::Dark,
            WindowAppearance::HighContrastDark => gpui::WindowAppearance::VibrantDark,
            WindowAppearance::Light | WindowAppearance::HighContrastLight => {
                gpui::WindowAppearance::Light
            }
        }
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.display.clone()
    }

    fn mouse_position(&self) -> gpui::Point<gpui::Pixels> {
        // Android is primarily touch-based; return a default position.
        // Touch events are delivered through the input callback.
        gpui::Point::default()
    }

    fn modifiers(&self) -> Modifiers {
        super::keyboard::android_meta_to_modifiers(self.window.state.lock().key_meta_state)
    }

    fn capslock(&self) -> Capslock {
        Capslock {
            on: self.window.state.lock().key_meta_state & super::keyboard::AMETA_CAPS_LOCK_ON != 0,
        }
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        // Explicit ownership is verified from the selected handler outside draw.
        super::jni::input_handler_changed();
        self.input_handler.set(Some(input_handler));
        super::jni::IME_HAS_HANDLER.store(true, std::sync::atomic::Ordering::Release);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        super::jni::input_handler_changed();
        self.input_handler.take()
    }

    fn set_text_input_configuration(&mut self, configuration: gpui::TextInputConfiguration) {
        super::jni::configure_text_input(configuration);
    }

    fn prompt(
        &self,
        _level: PromptLevel,
        msg: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        super::jni::show_prompt(msg, detail, answers)
    }

    fn activate(&self) {
        // Android windows are always "active" when in the foreground.
        // The system manages window activation via the activity lifecycle.
    }

    fn is_active(&self) -> bool {
        self.window.is_active()
    }

    fn visibility(&self) -> gpui::WindowVisibility {
        self.window.visibility()
    }

    fn on_visibility_change(&self, callback: Box<dyn FnMut(gpui::WindowVisibility)>) {
        // Matches the existing main-thread-only active/input callback bridge.
        let callback: Box<dyn FnMut(gpui::WindowVisibility) + Send> =
            unsafe { std::mem::transmute(callback) };
        *self.window.visibility_callback.lock() = Some(callback);
    }

    fn is_hovered(&self) -> bool {
        // Touch-based — always "hovered" when active.
        self.window.is_active()
    }

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        WindowBackgroundAppearance::Opaque
    }

    fn set_title(&mut self, title: &str) {
        self.title = title.to_string();
        // Android NativeActivity doesn't have a title bar to set.
        log::debug!("AndroidPlatformWindow::set_title({title})");
    }

    fn set_background_appearance(&self, _background: WindowBackgroundAppearance) {
        // Could toggle transparency on the renderer, but for now no-op.
    }

    fn minimize(&self) {
        // Android apps minimize via the system back/home button, not programmatically.
        log::debug!("AndroidPlatformWindow::minimize — no-op on Android");
    }

    fn zoom(&self) {
        // Android windows are always fullscreen.
    }

    fn toggle_fullscreen(&self) {
        // Android windows are always fullscreen.
    }

    fn is_fullscreen(&self) -> bool {
        true
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        // PlatformWindow gives us Box<dyn FnMut(...)> (not Send).
        // AndroidWindow::on_request_frame requires Send.  On Android the
        // request-frame callback is always invoked on the main thread, so
        // this transmute is safe in practice.
        let send_callback: Box<dyn FnMut(RequestFrameOptions) + Send> =
            unsafe { std::mem::transmute(callback) };
        let send_callback = Mutex::new(send_callback);

        // Also capture the input callback so we can emit momentum scroll
        // events before the GPUI render pass.  The input_callback is stored
        // as an Arc<Mutex<…>> by on_input — we clone the same Arc here.
        //
        // We need a reference to the shared momentum state and the shared
        // input callback so that the frame callback can pump inertia.
        let momentum = Arc::clone(&self.momentum);
        // The input_cb Arc is set up by on_input.  We store a clone of it
        // on the struct so on_request_frame can capture it.
        let input_cb = Arc::clone(&self.momentum_input_cb);

        self.window.on_request_frame(move || {
            // ── Drain coalesced touch-scroll deltas ──────────────────
            // The touch callback accumulates scroll deltas into
            // MomentumState rather than emitting ScrollWheel events
            // immediately.  We drain the accumulated delta here,
            // emitting at most ONE ScrollWheel event per frame.
            // This avoids redundant layout passes when Android
            // delivers many MOVE events between frames.
            if !crate::mobile_events::raw_touch_only() {
                let mut ms = momentum.lock();

                if ms.has_pending_scroll {
                    let dx = ms.pending_scroll_dx;
                    let dy = ms.pending_scroll_dy;
                    let pos_x = ms.pending_scroll_pos_x;
                    let pos_y = ms.pending_scroll_pos_y;
                    let phase = ms.pending_scroll_phase;

                    // Reset the accumulator.
                    ms.pending_scroll_dx = 0.0;
                    ms.pending_scroll_dy = 0.0;
                    ms.has_pending_scroll = false;

                    // Drop the lock before calling the input callback
                    // to avoid holding it during GPUI dispatch.
                    drop(ms);

                    let position = gpui::point(gpui::px(pos_x), gpui::px(pos_y));
                    if let Some(mut guard) = input_cb.try_lock() {
                        let _ = guard(gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                            position,
                            delta: gpui::ScrollDelta::Pixels(gpui::point(
                                gpui::px(dx),
                                gpui::px(dy),
                            )),
                            modifiers: gpui::Modifiers::default(),
                            touch_phase: phase,
                        }));
                    }
                } else if ms.scroller.is_active() {
                    // ── Momentum scrolling pump ──────────────────────
                    // No active touch drag — pump the momentum scroller.
                    if let Some(delta) = ms.scroller.step() {
                        let position =
                            gpui::point(gpui::px(delta.position_x), gpui::px(delta.position_y));
                        let fling_ended = !ms.scroller.is_active();

                        // Drop the lock before calling the input callback.
                        drop(ms);

                        if let Some(mut guard) = input_cb.try_lock() {
                            let _ =
                                guard(gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                                    position,
                                    delta: gpui::ScrollDelta::Pixels(gpui::point(
                                        gpui::px(delta.dx),
                                        gpui::px(delta.dy),
                                    )),
                                    modifiers: gpui::Modifiers::default(),
                                    touch_phase: gpui::TouchPhase::Moved,
                                }));

                            // If this was the last momentum frame (scroller
                            // deactivated during step), send the Ended event
                            // now so GPUI knows the gesture is complete.
                            if fling_ended {
                                let _ = guard(gpui::PlatformInput::ScrollWheel(
                                    gpui::ScrollWheelEvent {
                                        position,
                                        delta: gpui::ScrollDelta::Pixels(gpui::point(
                                            gpui::px(0.0),
                                            gpui::px(0.0),
                                        )),
                                        modifiers: gpui::Modifiers::default(),
                                        touch_phase: gpui::TouchPhase::Ended,
                                    },
                                ));
                            }
                        }
                    } else {
                        // Fling finished — emit a zero-delta Ended event.
                        let pos = gpui::point(
                            gpui::px(ms.scroller.position_x()),
                            gpui::px(ms.scroller.position_y()),
                        );
                        drop(ms);

                        if let Some(mut guard) = input_cb.try_lock() {
                            let _ =
                                guard(gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                                    position: pos,
                                    delta: gpui::ScrollDelta::Pixels(gpui::point(
                                        gpui::px(0.0),
                                        gpui::px(0.0),
                                    )),
                                    modifiers: gpui::Modifiers::default(),
                                    touch_phase: gpui::TouchPhase::Ended,
                                }));
                        }
                    }
                }
            }

            // Check if text input arrived since last frame — if so, force a
            // render so drain_pending_text() runs and the UI updates.
            let text_dirty =
                crate::TEXT_INPUT_DIRTY.swap(false, std::sync::atomic::Ordering::AcqRel);

            let mut cb = send_callback.lock();
            cb(RequestFrameOptions {
                require_presentation: true,
                force_render: text_dirty,
            });
        });
    }

    fn on_input(&self, callback: Box<dyn FnMut(gpui::PlatformInput) -> DispatchEventResult>) {
        // Bridge AndroidWindow touch/key callbacks → gpui::PlatformInput.
        //
        // PlatformWindow gives us Box<dyn FnMut(...)> (not Send).
        // AndroidWindow callbacks require Send.  On Android the input
        // callbacks are always invoked on the main thread, so this
        // transmute is safe in practice.
        let send_callback: Box<dyn FnMut(gpui::PlatformInput) -> DispatchEventResult + Send> =
            unsafe { std::mem::transmute(callback) };
        let input_cb = Arc::new(Mutex::new(send_callback));
        let ime_cb = input_cb.clone();
        let handler = self.input_handler.clone();
        let refresh_handler = handler.clone();
        let weak_window = Arc::downgrade(&self.window);
        let refresh: Box<dyn FnMut()> = Box::new(move || {
            let Some(window) = weak_window.upgrade() else {
                return;
            };
            let Some(handle) = window.gpui_handle() else {
                return;
            };
            let generation = refresh_handler.generation();
            let result = refresh_handler.call(
                || true,
                |candidate| {
                    crate::mobile_events::query_text_input_owner(|| {
                        candidate.query_accepts_text_input()
                    })
                },
            );
            let (accepts, owner) = if refresh_handler.generation() == generation {
                result.unwrap_or((false, None))
            } else {
                (false, None)
            };
            crate::mobile_events::bind_queried_text_input_owner(
                handle.window_id().as_u64(),
                accepts,
                owner,
            );
        });
        // Like ime_callback, only invoked on the native GPUI owner thread.
        self.window.state.lock().ime_owner_refresh = Some(unsafe { std::mem::transmute(refresh) });
        let callback: Box<dyn FnMut(i64, i64, crate::mobile_events::ImeInput)> =
            Box::new(move |session, editor, input| {
                if !super::jni::ime_delivery_current(session, editor) {
                    return;
                }
                if let Some(lease) = crate::mobile_events::active_text_input_lease() {
                    let generation = handler.generation();
                    handler.call(
                        || super::jni::ime_delivery_current(session, editor),
                        |candidate| {
                            let Some(input) =
                                crate::mobile_events::verify_owned_ime(lease, input, || {
                                    candidate.query_accepts_text_input()
                                })
                            else {
                                return;
                            };
                            let current = || {
                                super::jni::ime_delivery_current(session, editor)
                                    && handler.generation() == generation
                                    && crate::mobile_events::active_text_input_lease()
                                        == Some(lease)
                            };
                            let verify = |candidate: &mut PlatformInputHandler| {
                                let (accepts, owner) =
                                    crate::mobile_events::query_text_input_owner(|| {
                                        candidate.query_accepts_text_input()
                                    });
                                accepts
                                    && current()
                                    && crate::mobile_events::queried_owner_matches(lease, owner)
                            };
                            if !current() {
                                return;
                            }
                            use crate::mobile_events::ImeInput;
                            match input {
                                ImeInput::Commit(text) => {
                                    candidate.replace_text_in_range(None, &text)
                                }
                                ImeInput::Compose { text, selection } => candidate
                                    .replace_and_mark_text_in_range(None, &text, Some(selection)),
                                ImeInput::FinishComposition => candidate.unmark_text(),
                                ImeInput::Enter | ImeInput::Backspace => {
                                    let enter = matches!(input, ImeInput::Enter);
                                    let result = ime_cb.lock()(gpui::PlatformInput::KeyDown(
                                        gpui::KeyDownEvent {
                                            keystroke: gpui::Keystroke {
                                                modifiers: Default::default(),
                                                key: if enter { "enter" } else { "backspace" }
                                                    .into(),
                                                key_char: None,
                                            },
                                            is_held: false,
                                            prefer_character_input: false,
                                        },
                                    ));
                                    if enter
                                        && result.propagate
                                        && !result.default_prevented
                                        && verify(candidate)
                                    {
                                        candidate.replace_text_in_range(None, "\n");
                                    }
                                }
                                ImeInput::DeleteBefore { count, codepoints } => {
                                    let Some(selection) = candidate.selected_text_range(true)
                                    else {
                                        return;
                                    };
                                    if !verify(candidate) {
                                        return;
                                    }
                                    if !selection.range.is_empty() {
                                        candidate.replace_text_in_range(Some(selection.range), "");
                                        return;
                                    }
                                    let end = selection.range.start;
                                    let start = end.saturating_sub(count.saturating_mul(2));
                                    let mut adjusted = None;
                                    let Some(text) =
                                        candidate.text_for_range(start..end, &mut adjusted)
                                    else {
                                        return;
                                    };
                                    if adjusted.unwrap_or(start..end).end != end
                                        || !verify(candidate)
                                    {
                                        return;
                                    }
                                    if let Some(units) = crate::mobile_events::ime_delete_units(
                                        &text, count, codepoints,
                                    ) {
                                        candidate.replace_text_in_range(
                                            Some(end.saturating_sub(units)..end),
                                            "",
                                        );
                                    }
                                }
                            }
                        },
                    );
                    return;
                }
                if let crate::mobile_events::ImeInput::DeleteBefore { count, codepoints } = &input {
                    let (count, codepoints) = (*count, *codepoints);
                    handler.call(
                        || super::jni::ime_delivery_current(session, editor),
                        |handler| {
                            if !handler.query_accepts_text_input() {
                                return;
                            }
                            let Some(selection) = handler.selected_text_range(true) else {
                                return;
                            };
                            // The proxy has no document mirror. Query only enough real editor
                            // context for this operation and retain Android's unit distinction.
                            let end = selection.range.start;
                            let start = end.saturating_sub(count.saturating_mul(2));
                            let mut adjusted = None;
                            let Some(text) = handler.text_for_range(start..end, &mut adjusted)
                            else {
                                return;
                            };
                            let actual = adjusted.unwrap_or(start..end);
                            if actual.end != end {
                                return;
                            }
                            let Some(units) =
                                crate::mobile_events::ime_delete_units(&text, count, codepoints)
                            else {
                                return;
                            };
                            if !super::jni::ime_delivery_current(session, editor) {
                                return;
                            }
                            handler.replace_text_in_range(Some(end.saturating_sub(units)..end), "");
                        },
                    );
                    return;
                }
                crate::mobile_dispatch::deliver_ime_key(
                    input,
                    || super::jni::ime_delivery_current(session, editor),
                    crate::dispatch_text_input,
                    |input| ime_cb.lock()(input),
                    |text| {
                        handler.call(
                            || super::jni::ime_delivery_current(session, editor),
                            |handler| {
                                let accepts = handler.query_accepts_text_input();
                                let current = super::jni::ime_delivery_current(session, editor);
                                log::info!("IME handler accepts={accepts} current={current}");
                                if accepts && current {
                                    handler.replace_text_in_range(None, text);
                                    log::info!("IME handler replacement returned");
                                }
                            },
                        );
                    },
                );
            });
        // Invoked only by JNI queue draining on this native GPUI thread, like key_callback.
        self.window.state.lock().ime_callback = Some(unsafe { std::mem::transmute(callback) });

        // Store a clone for the momentum pump in on_request_frame.
        *self.momentum_input_cb.lock() = {
            let cb = Arc::clone(&input_cb);
            Box::new(move |input: gpui::PlatformInput| -> DispatchEventResult { cb.lock()(input) })
        };

        // ── Touch events → PlatformInput ─────────────────────────────────
        //
        // Android touch events must be translated into both mouse events
        // (for taps / clicks) and scroll-wheel events (for drag-to-scroll).
        //
        // A small state machine distinguishes the two gestures:
        //
        //   DOWN  → record start position, enter "pending" state
        //   MOVE  → if finger moved > threshold → switch to "scrolling",
        //           cancel the mouse-down, emit ScrollWheel deltas
        //   UP    → if still "pending" → emit MouseDown + MouseUp (tap)
        //           if "scrolling"   → emit final ScrollWheel (Ended) +
        //           start momentum fling
        //
        // The threshold is in logical pixels (~8 px ≈ ~3 mm at 160 dpi).
        {
            let cb = Arc::clone(&input_cb);
            let window = Arc::downgrade(&self.window);
            let momentum = Arc::clone(&self.momentum);

            /// Distance (logical px) the finger must travel before a touch
            /// is promoted from a potential tap to a scroll gesture.
            const SCROLL_SLOP: f32 = 8.0;

            /// Tracks the current touch gesture.
            #[derive(Clone, Copy, Debug)]
            enum TouchState {
                /// No active touch.
                Idle,
                /// Finger is down but hasn't moved beyond the slop threshold.
                Pending { start_x: f32, start_y: f32 },
                /// Finger has moved beyond the threshold — we are scrolling.
                Scrolling { prev_x: f32, prev_y: f32 },
            }

            let state = Mutex::new(TouchState::Idle);

            self.window.on_touch(move |touch| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let scale_factor = window.scale_factor();
                // Android delivers touch coordinates in physical (device)
                // pixels, but GPUI performs layout and hit-testing in logical
                // pixels.  Divide by scale factor.
                let logical_x = touch.x / scale_factor;
                let logical_y = touch.y / scale_factor;
                let modifiers = gpui::Modifiers::default();

                let mut ts = state.lock();

                match touch.action {
                    // ── ACTION_DOWN ──────────────────────────────────────
                    0 => {
                        // Cancel any active momentum fling — the user
                        // touched the screen, so inertia must stop.
                        // Also flush any pending coalesced scroll.
                        {
                            let mut ms = momentum.lock();
                            ms.scroller.cancel();
                            ms.velocity_tracker.reset();
                            ms.pending_scroll_dx = 0.0;
                            ms.pending_scroll_dy = 0.0;
                            ms.has_pending_scroll = false;
                        }
                        *ts = TouchState::Pending {
                            start_x: logical_x,
                            start_y: logical_y,
                        };
                        // Do NOT emit MouseDown here — wait until we know
                        // whether this is a tap or a scroll.  Emitting
                        // MouseDown immediately causes accidental navigation
                        // when the user starts scrolling near a button/tab.
                        //
                        // - Tap (finger lifts within slop) → emit MouseDown +
                        //   MouseUp together in ACTION_UP.
                        // - Scroll (finger exceeds slop) → emit only
                        //   MouseMove + ScrollWheel, no MouseDown.
                    }

                    // ── ACTION_MOVE ──────────────────────────────────────
                    2 => {
                        // Instead of emitting a ScrollWheel event for every
                        // single MOVE, accumulate the delta in MomentumState.
                        // The frame callback will drain and emit one coalesced
                        // ScrollWheel per frame.  This is the key optimisation
                        // that prevents N layout passes per frame during a drag.
                        //
                        // We DO emit MouseMove immediately for every MOVE so
                        // that interactive screens (Animations drag line,
                        // Shaders touch position) update in real time.
                        let mut ms = momentum.lock();

                        // Record every move for velocity estimation.
                        ms.velocity_tracker.record(logical_x, logical_y);

                        match *ts {
                            TouchState::Pending { start_x, start_y } => {
                                let dx = logical_x - start_x;
                                let dy = logical_y - start_y;
                                let distance = (dx * dx + dy * dy).sqrt();

                                if distance > SCROLL_SLOP {
                                    // Promote to scrolling — accumulate the
                                    // first scroll delta from the start pos.
                                    *ts = TouchState::Scrolling {
                                        prev_x: logical_x,
                                        prev_y: logical_y,
                                    };
                                    ms.pending_scroll_dx += dx;
                                    ms.pending_scroll_dy += dy;
                                    ms.pending_scroll_pos_x = logical_x;
                                    ms.pending_scroll_pos_y = logical_y;
                                    // Use Started phase for the first batch.
                                    if !ms.has_pending_scroll {
                                        ms.pending_scroll_phase = gpui::TouchPhase::Started;
                                    }
                                    ms.has_pending_scroll = true;
                                }
                                // else: still within slop, stay Pending
                            }
                            TouchState::Scrolling { prev_x, prev_y } => {
                                let dx = logical_x - prev_x;
                                let dy = logical_y - prev_y;
                                *ts = TouchState::Scrolling {
                                    prev_x: logical_x,
                                    prev_y: logical_y,
                                };
                                ms.pending_scroll_dx += dx;
                                ms.pending_scroll_dy += dy;
                                ms.pending_scroll_pos_x = logical_x;
                                ms.pending_scroll_pos_y = logical_y;
                                if !ms.has_pending_scroll {
                                    ms.pending_scroll_phase = gpui::TouchPhase::Moved;
                                }
                                ms.has_pending_scroll = true;
                            }
                            TouchState::Idle => {
                                // Spurious move without a preceding down — ignore.
                            }
                        }

                        // Drop momentum lock before dispatching MouseMove.
                        drop(ms);

                        // Always emit MouseMove so interactive screens can
                        // track finger position (drag line in Animations,
                        // gradient control in Shaders).
                        let position = gpui::point(gpui::px(logical_x), gpui::px(logical_y));
                        let mut guard = cb.lock();
                        let _ = guard(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                            position,
                            modifiers,
                            pressed_button: Some(gpui::MouseButton::Left),
                        }));
                    }

                    3 => {
                        let was_scrolling = matches!(*ts, TouchState::Scrolling { .. });
                        let position = match *ts {
                            TouchState::Scrolling { prev_x, prev_y } => {
                                gpui::point(gpui::px(prev_x), gpui::px(prev_y))
                            }
                            _ => Default::default(),
                        };
                        *ts = TouchState::Idle;
                        let mut ms = momentum.lock();
                        ms.scroller.cancel();
                        ms.velocity_tracker.reset();
                        ms.pending_scroll_dx = 0.0;
                        ms.pending_scroll_dy = 0.0;
                        ms.has_pending_scroll = false;
                        drop(ms);
                        if was_scrolling {
                            cb.lock()(gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                                position,
                                delta: gpui::ScrollDelta::Pixels(Default::default()),
                                modifiers,
                                touch_phase: gpui::TouchPhase::Cancelled,
                            }));
                        }
                    }
                    // ── ACTION_UP ────────────────────────────────────────
                    1 => {
                        let position = gpui::point(gpui::px(logical_x), gpui::px(logical_y));

                        match *ts {
                            TouchState::Pending { start_x, start_y } => {
                                // Finger lifted without exceeding slop →
                                // this is a tap.  Emit MouseDown + MouseUp
                                // together at the original down position so
                                // hit-testing matches the initial touch point.
                                {
                                    let mut ms = momentum.lock();
                                    ms.velocity_tracker.reset();
                                    ms.has_pending_scroll = false;
                                }
                                let tap_pos = gpui::point(gpui::px(start_x), gpui::px(start_y));
                                let mut guard = cb.lock();
                                let _ =
                                    guard(gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                                        button: gpui::MouseButton::Left,
                                        position: tap_pos,
                                        modifiers,
                                        click_count: 1,
                                        first_mouse: false,
                                    }));
                                let _ = guard(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                                    button: gpui::MouseButton::Left,
                                    position: tap_pos,
                                    modifiers,
                                    click_count: 1,
                                }));
                            }
                            TouchState::Scrolling { prev_x, prev_y } => {
                                // End the active touch-scroll gesture.
                                // Include the final delta in the coalesced
                                // accumulator, then flush it immediately
                                // as an Ended event so the momentum fling
                                // starts cleanly.
                                let dx = logical_x - prev_x;
                                let dy = logical_y - prev_y;
                                let mut ms = momentum.lock();

                                // Flush any accumulated delta + this final
                                // move as a single Ended scroll event.
                                let total_dx = ms.pending_scroll_dx + dx;
                                let total_dy = ms.pending_scroll_dy + dy;
                                ms.pending_scroll_dx = 0.0;
                                ms.pending_scroll_dy = 0.0;
                                ms.has_pending_scroll = false;

                                // Compute release velocity and start fling.
                                let (vx, vy) = ms.velocity_tracker.velocity();
                                ms.velocity_tracker.reset();
                                ms.scroller.fling(vx, vy, logical_x, logical_y);

                                // Drop momentum lock before dispatching.
                                drop(ms);

                                let mut guard = cb.lock();
                                // ScrollWheel Ended for scroll containers.
                                let _ = guard(gpui::PlatformInput::ScrollWheel(
                                    gpui::ScrollWheelEvent {
                                        position,
                                        delta: gpui::ScrollDelta::Pixels(gpui::point(
                                            gpui::px(total_dx),
                                            gpui::px(total_dy),
                                        )),
                                        modifiers,
                                        touch_phase: gpui::TouchPhase::Ended,
                                    },
                                ));
                                // MouseUp for interactive screens (Animations
                                // drag-to-throw, Shaders touch release).
                                let _ = guard(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                                    button: gpui::MouseButton::Left,
                                    position,
                                    modifiers,
                                    click_count: 1,
                                }));
                            }
                            TouchState::Idle => {}
                        }
                        *ts = TouchState::Idle;
                    }

                    _ => {} // Unknown action, ignore
                }
            });
        }

        // ── Key events → PlatformInput ───────────────────────────────────
        {
            let cb = Arc::clone(&input_cb);
            self.window.on_key_event(move |key_event| {
                use crate::android::keyboard::{
                    android_key_to_keystroke, AKEY_EVENT_ACTION_DOWN, AKEY_EVENT_ACTION_UP,
                };

                // On KeyDown, dispatch text through the global callback so
                // custom TextInput components (PENDING_TEXT) receive it.
                if key_event.action == AKEY_EVENT_ACTION_DOWN
                    && crate::android::keyboard::allows_text_input(key_event.meta_state)
                {
                    match key_event.key_code {
                        67 => crate::dispatch_text_input("\x08"), // KEYCODE_DEL (backspace)
                        21 => crate::dispatch_text_input("\x1b[D"), // DPAD_LEFT
                        22 => crate::dispatch_text_input("\x1b[C"), // DPAD_RIGHT
                        122 => crate::dispatch_text_input("\x1b[H"), // MOVE_HOME
                        123 => crate::dispatch_text_input("\x1b[F"), // MOVE_END
                        _ => {
                            if key_event.unicode_char != 0 {
                                if let Some(c) = char::from_u32(key_event.unicode_char) {
                                    let s = c.to_string();
                                    crate::dispatch_text_input(&s);
                                }
                            }
                            false
                        }
                    };
                }

                let keystroke = match android_key_to_keystroke(
                    key_event.key_code,
                    key_event.meta_state,
                    key_event.unicode_char,
                ) {
                    Some(ks) => ks,
                    None => return, // Modifier-only or unmapped key
                };

                let event = if key_event.action == AKEY_EVENT_ACTION_DOWN {
                    gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                        keystroke,
                        is_held: false,
                        prefer_character_input: key_event.unicode_char != 0,
                    })
                } else if key_event.action == AKEY_EVENT_ACTION_UP {
                    gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke })
                } else {
                    return; // ACTION_MULTIPLE or unknown
                };

                let mut guard = cb.lock();
                let _ = guard(event);
            });
        }
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        // Wire the callback to AndroidWindow's active status change.
        // PlatformWindow gives us Box<dyn FnMut(bool)> (not Send).
        // AndroidWindow::on_active_status_change requires Send.
        // On Android, this callback is always invoked on the main thread.
        let send_callback: Box<dyn FnMut(bool) + Send> = unsafe { std::mem::transmute(callback) };
        let send_callback = Mutex::new(send_callback);
        self.window.on_active_status_change(move |active| {
            log::info!("GPUI active-status delivery active={active}");
            let mut cb = send_callback.lock();
            cb(active);
        });
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        let _callback = Mutex::new(callback);
        // No hover concept on touch devices
    }

    fn on_resize(&self, callback: Box<dyn FnMut(gpui::Size<gpui::Pixels>, f32)>) {
        // PlatformWindow::on_resize gives us Box<dyn FnMut(...)> (not Send).
        // AndroidWindow::on_resize requires Send.  On Android the resize
        // callback is always invoked on the main thread, so this is safe.
        let send_callback: Box<dyn FnMut(gpui::Size<gpui::Pixels>, f32) + Send> =
            unsafe { std::mem::transmute(callback) };
        let send_callback = Arc::new(Mutex::new(send_callback));
        self.window.on_resize(move |device_size, scale| {
            let mut cb = send_callback.lock();
            cb(
                gpui::size(
                    gpui::px(device_size.width.0 as f32 / scale),
                    gpui::px(device_size.height.0 as f32 / scale),
                ),
                scale,
            );
        });
    }

    fn on_moved(&self, _callback: Box<dyn FnMut()>) {
        // Android windows don't move — they're always fullscreen.
    }

    fn on_should_close(&self, _callback: Box<dyn FnMut() -> bool>) {
        // Android app lifecycle is managed by the system.
    }

    fn on_hit_test_window_control(&self, _callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
        // No window controls on Android.
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        // PlatformWindow gives us Box<dyn FnOnce()> (not Send).
        // AndroidWindow::on_close requires Send.  On Android, the close
        // callback is always invoked on the main thread, so this transmute
        // is safe in practice.
        let send_callback: Box<dyn FnOnce() + Send + 'static> =
            unsafe { std::mem::transmute::<Box<dyn FnOnce()>, Box<dyn FnOnce() + Send>>(callback) };
        self.window.on_close(send_callback);
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        // Wire to system dark mode changes via Configuration.
        // PlatformWindow gives us Box<dyn FnMut()> (not Send).
        // AndroidWindow::on_appearance_changed requires Send.
        // On Android, this callback is always invoked on the main thread.
        let send_callback: Box<dyn FnMut() + Send> = unsafe { std::mem::transmute(callback) };
        let send_callback = Mutex::new(send_callback);
        self.window.on_appearance_changed(move |_appearance| {
            let mut cb = send_callback.lock();
            cb();
        });
    }

    fn draw(&self, scene: &gpui::Scene) {
        // gpui_wgpu::WgpuRenderer natively consumes gpui::Scene — no bridging needed.
        log::trace!(
            "AndroidPlatformWindow::draw — {} quads, {} shadows",
            scene.quads.len(),
            scene.shadows.len(),
        );

        self.window.draw(scene);
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.window
            .sprite_atlas()
            .unwrap_or_else(|| Arc::new(FallbackAtlas::new()))
    }

    fn is_subpixel_rendering_supported(&self) -> bool {
        self.window.supports_subpixel_aa()
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.window.gpu_specs()
    }

    fn text_input_state_changed(&self, change: gpui::TextInputStateChange) {
        // GPUI temporarily takes/restores handlers during painting. Only actual
        // focus transitions request IME changes through the existing helpers.
        match change {
            gpui::TextInputStateChange::FocusGained => {
                super::jni::show_keyboard_android(crate::KeyboardType::Default);
            }
            gpui::TextInputStateChange::FocusLost => super::jni::hide_keyboard_android(),
            gpui::TextInputStateChange::SelectionChanged
            | gpui::TextInputStateChange::ContentChanged => {}
        }
    }

    fn update_ime_position(&self, bounds: gpui::Bounds<gpui::Pixels>) {
        use crate::android::jni as jni_helpers;
        use jni::objects::JValue;

        let x: f32 = bounds.origin.x.into();
        let y: f32 = bounds.origin.y.into();
        let height: f32 = bounds.size.height.into();
        let density = self.window.scale_factor();
        if ![x, y, height, density]
            .iter()
            .all(|value| value.is_finite())
            || height < 0.0
            || density <= 0.0
        {
            return;
        }
        if let Some(app) = jni_helpers::android_app() {
            app.run_on_java_main_thread(Box::new(move || {
                if let Err(error) = jni_helpers::with_env(|env| {
                    let class = jni_helpers::find_app_class(env, "dev.gpui.mobile.GpuiInsets")?;
                    let activity = jni_helpers::activity(env)?;
                    env.call_static_method(
                        &class,
                        jni::jni_str!("updateCursor"),
                        jni::jni_sig!("(Landroid/app/Activity;FFFF)V"),
                        &[
                            JValue::Object(&activity),
                            JValue::Float(x),
                            JValue::Float(y),
                            JValue::Float(height),
                            JValue::Float(density),
                        ],
                    )
                    .map(|_| ())
                    .map_err(|error| {
                        env.exception_clear();
                        error.to_string()
                    })
                }) {
                    log::warn!("update_ime_position: {error}");
                }
            }));
        }
    }
}

// ── Fallback atlas ────────────────────────────────────────────────────────────

/// A minimal fallback `PlatformAtlas` used only when the renderer is not
/// available (e.g. before the wgpu surface is created).  Once the
/// `gpui_wgpu::WgpuRenderer` is initialised, `sprite_atlas()` returns the
/// real `WgpuAtlas` from the renderer instead.
struct FallbackAtlas {
    state: Mutex<FallbackAtlasState>,
}

struct FallbackAtlasState {
    next_id: u32,
    tiles: HashMap<AtlasKey, AtlasTile>,
}

impl FallbackAtlas {
    fn new() -> Self {
        Self {
            state: Mutex::new(FallbackAtlasState {
                next_id: 1,
                tiles: HashMap::new(),
            }),
        }
    }
}

impl PlatformAtlas for FallbackAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> anyhow::Result<
            Option<(gpui::Size<gpui::DevicePixels>, std::borrow::Cow<'a, [u8]>)>,
        >,
    ) -> anyhow::Result<Option<AtlasTile>> {
        let mut state = self.state.lock();

        if let Some(tile) = state.tiles.get(&key) {
            return Ok(Some(tile.clone()));
        }

        let data = build()?;
        if let Some((size, _pixels)) = data {
            let id = state.next_id;
            state.next_id += 1;

            let tile = AtlasTile {
                texture_id: gpui::AtlasTextureId {
                    index: 0,
                    kind: gpui::AtlasTextureKind::Monochrome,
                },
                tile_id: gpui::TileId(id),
                padding: 0,
                bounds: gpui::Bounds {
                    origin: gpui::point(gpui::DevicePixels(0), gpui::DevicePixels(0)),
                    size,
                },
            };

            state.tiles.insert(key.clone(), tile.clone());
            Ok(Some(tile))
        } else {
            Ok(None)
        }
    }

    fn remove(&self, key: &AtlasKey) {
        self.state.lock().tiles.remove(key);
    }
}

// ── WindowList helper ─────────────────────────────────────────────────────────

/// Tracks all live `AndroidWindow` instances in the process.
///
/// On a typical Android device there is at most one window, but the list
/// supports more for forward compatibility.
#[derive(Default)]
pub struct WindowList {
    windows: Vec<Arc<AndroidWindow>>,
}

impl WindowList {
    /// Add a window to the list.
    pub fn push(&mut self, window: Arc<AndroidWindow>) {
        self.windows.push(window);
    }

    /// Remove and return the window with the given `id`, if present.
    pub fn remove(&mut self, id: u64) -> Option<Arc<AndroidWindow>> {
        if let Some(pos) = self.windows.iter().position(|w| w.id() == id) {
            Some(self.windows.remove(pos))
        } else {
            None
        }
    }

    /// Find a window by id (shared reference).
    pub fn get(&self, id: u64) -> Option<&Arc<AndroidWindow>> {
        self.windows.iter().find(|w| w.id() == id)
    }

    /// Returns the first (primary) window, if any.
    pub fn primary(&self) -> Option<&Arc<AndroidWindow>> {
        self.windows.first()
    }

    /// Iterate over all windows.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<AndroidWindow>> {
        self.windows.iter()
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_window_geometry() {
        let w = AndroidWindow::headless(1080, 1920, 3.0);
        assert_eq!(w.physical_size().width, DevicePixels(1080));
        assert_eq!(w.physical_size().height, DevicePixels(1920));
        assert!((w.scale_factor() - 3.0).abs() < f32::EPSILON);
        assert!(!w.has_surface());
        assert!(!w.is_active());
    }

    #[test]
    fn headless_window_logical_size() {
        let w = AndroidWindow::headless(1080, 1920, 3.0);
        let ls = w.logical_size();
        assert!((ls.width.0 - 360.0).abs() < f32::EPSILON);
        assert!((ls.height.0 - 640.0).abs() < f32::EPSILON);
    }

    #[test]
    fn headless_window_bounds_origin_is_zero() {
        let w = AndroidWindow::headless(1080, 1920, 2.0);
        let b = w.bounds();
        assert_eq!(b.origin.x, DevicePixels(0));
        assert_eq!(b.origin.y, DevicePixels(0));
    }

    #[test]
    fn window_id_is_stable() {
        let w = AndroidWindow::headless(1080, 1920, 2.0);
        assert_eq!(w.id(), w.id());
    }

    #[test]
    fn window_appearance_defaults_to_light() {
        let w = AndroidWindow::headless(1080, 1920, 2.0);
        assert_eq!(w.appearance(), WindowAppearance::Light);
    }

    #[test]
    fn window_set_appearance_dark() {
        let w = AndroidWindow::headless(1080, 1920, 2.0);
        w.set_appearance(WindowAppearance::Dark);
        assert_eq!(w.appearance(), WindowAppearance::Dark);
    }

    #[test]
    fn window_appearance_callback_fires() {
        let w = AndroidWindow::headless(1080, 1920, 2.0);

        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fired_clone = fired.clone();

        w.on_appearance_changed(move |_| {
            fired_clone.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        w.set_appearance(WindowAppearance::Dark);
        assert!(fired.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn window_appearance_callback_not_fired_if_unchanged() {
        let w = AndroidWindow::headless(1080, 1920, 2.0);
        // Default is Light; setting Light again should not fire.
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fired_clone = fired.clone();

        w.on_appearance_changed(move |_| {
            fired_clone.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        w.set_appearance(WindowAppearance::Light);
        assert!(!fired.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn touch_callback_fires() {
        let w = AndroidWindow::headless(1080, 1920, 2.0);
        w.mobile_events
            .lock()
            .surface(SurfaceLifecycle::Available, std::time::Duration::ZERO);
        let received = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<TouchPoint>::new()));
        let r2 = received.clone();

        w.on_touch(move |pt| {
            r2.lock().push(pt);
        });

        w.handle_touch(TouchPoint {
            id: 1,
            x: 100.0,
            y: 200.0,
            action: 0,
        });

        let pts = received.lock();
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0].id, 1);
    }

    #[test]
    fn key_callback_fires() {
        let w = AndroidWindow::headless(1080, 1920, 2.0);
        let received = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<AndroidKeyEvent>::new()));
        let r2 = received.clone();

        w.on_key_event(move |e| {
            r2.lock().push(e);
        });

        w.handle_key_event(AndroidKeyEvent {
            key_code: 29, // KEYCODE_A
            action: 0,
            meta_state: 0,
            unicode_char: b'a' as u32,
        });

        let events = received.lock();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key_code, 29);
    }

    #[test]
    fn request_frame_requires_visible_surface() {
        let w = AndroidWindow::headless(1080, 1920, 2.0);
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c2 = count.clone();

        w.on_request_frame(move || {
            c2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });

        w.request_frame();
        assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 0);
        w.app_resumed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        w.activity.set_surface(true);
        w.request_frame();
        w.request_frame();

        assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn window_list_push_get_remove() {
        let mut list = WindowList::default();
        let w = AndroidWindow::headless(1080, 1920, 2.0);
        let id = w.id();

        list.push(w);
        assert_eq!(list.len(), 1);
        assert!(list.get(id).is_some());

        let removed = list.remove(id);
        assert!(removed.is_some());
        assert!(list.is_empty());
        assert!(list.get(id).is_none());
    }

    #[test]
    fn window_list_primary() {
        let mut list = WindowList::default();
        list.push(AndroidWindow::headless(1080, 1920, 2.0));
        assert!(list.primary().is_some());
    }

    #[test]
    fn window_list_remove_missing_returns_none() {
        let mut list = WindowList::default();
        assert!(list.remove(0xDEADBEEF).is_none());
    }

    #[test]
    fn subpixel_aa_false_for_headless() {
        let w = AndroidWindow::headless(1080, 1920, 2.0);
        assert!(!w.supports_subpixel_aa());
    }

    #[test]
    fn gpu_info_none_for_headless() {
        let w = AndroidWindow::headless(1080, 1920, 2.0);
        assert!(w.gpu_info().is_none());
    }
}
