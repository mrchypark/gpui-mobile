//! iOS platform view implementation.
//!
//! Embeds native iOS `UIView` instances in the GPUI render tree using
//! hybrid composition. Native views are created here and can be inserted
//! into the view hierarchy by the iOS window code when a platform view
//! element is painted.

use crate::platform_view::{
    PlatformView, PlatformViewBounds, PlatformViewFactory, PlatformViewId, PlatformViewParams,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "ios")]
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

#[cfg(target_os = "ios")]
use super::cg_types::ObjcCGRect;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub(crate) struct NativeView {
    pub pointer: usize,
    view_type: String,
    // MKMapView retains these annotations; this table only indexes them.
    pub markers: HashMap<String, usize>,
}

fn native_views() -> &'static Mutex<HashMap<PlatformViewId, NativeView>> {
    static VIEWS: OnceLock<Mutex<HashMap<PlatformViewId, NativeView>>> = OnceLock::new();
    VIEWS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Borrow the actual paint window's UIKit view. No global-window ordering or
/// key-window state participates in attachment, and no window pointer is cached.
pub(crate) fn attach_to_paint_window(
    id: PlatformViewId,
    window: &gpui::Window,
) -> Result<(), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let main: bool = unsafe { msg_send![class!(NSThread), isMainThread] };
    if !main {
        return Err("Platform view paint must run on the iOS main thread".into());
    }
    let handle = HasWindowHandle::window_handle(window).map_err(|e| e.to_string())?;
    let RawWindowHandle::UiKit(handle) = handle.as_raw() else {
        return Err("Paint window does not have a UIKit view".into());
    };
    let pointer = native_views()
        .lock()
        .map_err(|_| "Native view registry poisoned")?
        .get(&id)
        .ok_or("Native view is disposed or missing")?
        .pointer;
    unsafe { attach_above_metal(pointer as *mut AnyObject, handle.ui_view.as_ptr().cast()) }
}

unsafe fn attach_above_metal(view: *mut AnyObject, metal: *mut AnyObject) -> Result<(), String> {
    let parent: *mut AnyObject = msg_send![metal, superview];
    if parent.is_null() {
        return Err("Paint window's Metal view has no superview".into());
    }
    let current: *mut AnyObject = msg_send![view, superview];
    if current != parent {
        // UIKit reparents the view if this element has moved to another window.
        let _: () = msg_send![parent, insertSubview: view, aboveSubview: metal];
    }
    Ok(())
}

pub(crate) fn on_main<R: Send, F: FnOnce() -> R + Send>(action: F) -> R {
    unsafe extern "C" fn invoke<F: FnOnce() -> R, R>(context: *mut std::ffi::c_void) {
        let state = &mut *(context as *mut (Option<F>, Option<std::thread::Result<R>>));
        state.1 = Some(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            state.0.take().unwrap(),
        )));
    }
    unsafe extern "C" {
        fn dispatch_sync_f(
            queue: *mut std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: unsafe extern "C" fn(*mut std::ffi::c_void),
        );
    }
    let main: bool = unsafe { msg_send![class!(NSThread), isMainThread] };
    if main {
        return action();
    }
    let mut state: (Option<F>, Option<std::thread::Result<R>>) = (Some(action), None);
    unsafe {
        dispatch_sync_f(
            super::dispatcher::dispatch_get_main_queue(),
            &mut state as *mut _ as *mut _,
            invoke::<F, R>,
        );
    }
    match state.1.unwrap() {
        Ok(value) => value,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

pub(crate) fn with_native<R: Send>(
    id: PlatformViewId,
    kind: &str,
    action: impl FnOnce(&mut NativeView) -> Result<R, String> + Send,
) -> Result<R, String> {
    on_main(|| {
        let mut views = native_views()
            .lock()
            .map_err(|_| "Native view registry poisoned")?;
        let view = views
            .get_mut(&id)
            .ok_or("Native view is disposed or missing")?;
        if view.view_type != kind {
            return Err("Native view type mismatch".into());
        }
        action(view)
    })
}

/// iOS implementation of a platform view.
///
/// Wraps a `UIView` instance. The view is created during construction but
/// is NOT automatically added to the view hierarchy. Call
/// `native_view_ptr()` to get the raw `*mut AnyObject` pointer, then insert
/// it into the appropriate superview from the iOS window code.
///
/// The element's paint callback attaches it to that exact paint window.
pub struct IosPlatformView {
    id: PlatformViewId,
    view_type: String,
    /// Pointer to the Objective-C UIView instance.
    /// Null if disposed.
    #[cfg(target_os = "ios")]
    native_view: std::sync::Mutex<*mut AnyObject>,
    disposed: AtomicBool,
    bounds: std::sync::Mutex<PlatformViewBounds>,
}

// Safety: UIView operations are dispatched to the main thread.
unsafe impl Send for IosPlatformView {}
unsafe impl Sync for IosPlatformView {}

impl IosPlatformView {
    /// Create a new iOS platform view.
    ///
    /// The UIView is allocated and initialized with the given bounds, but
    /// is NOT added to any view hierarchy. The caller is responsible for
    /// inserting the view by using `native_view_ptr()`.
    #[cfg(target_os = "ios")]
    pub fn new(view_type: &str, params: &PlatformViewParams) -> Result<Self, String> {
        let id = PlatformViewId::next();

        let native_view = on_main(|| {
            let pointer = Self::create_native_view(view_type, &params.bounds, params)? as usize;
            native_views().lock().unwrap().insert(
                id,
                NativeView {
                    pointer,
                    view_type: view_type.into(),
                    markers: HashMap::new(),
                },
            );
            Ok::<_, String>(pointer)
        })? as *mut AnyObject;

        Ok(Self {
            id,
            view_type: view_type.to_string(),
            native_view: std::sync::Mutex::new(native_view),
            disposed: AtomicBool::new(false),
            bounds: std::sync::Mutex::new(params.bounds),
        })
    }

    /// Create the native UIView via Objective-C runtime.
    ///
    /// Dispatches to type-specific creation for known view types:
    /// - "video_player": Creates UIView with AVPlayerLayer (requires player_id in params)
    /// - "webview": Creates WKWebView (uses url/html from params)
    /// - "camera_preview": Creates UIView with AVCaptureVideoPreviewLayer (requires session_id in params)
    /// - Default: Creates a generic UIView container
    #[cfg(target_os = "ios")]
    fn create_native_view(
        view_type: &str,
        bounds: &PlatformViewBounds,
        params: &PlatformViewParams,
    ) -> Result<*mut AnyObject, String> {
        unsafe {
            let frame = ObjcCGRect::new(
                bounds.x as f64,
                bounds.y as f64,
                bounds.width as f64,
                bounds.height as f64,
            );

            let view: *mut AnyObject = match view_type {
                "video_player" => Self::create_video_player_view(frame, params)?,
                "webview" => Self::create_webview_view(frame, params)?,
                #[cfg(feature = "maps")]
                "map" => crate::packages::maps::ios::create(frame, params)?,
                "camera_preview" => Self::create_camera_preview_view(frame, params)?,
                _ => Self::create_generic_view(frame)?,
            };

            if view.is_null() {
                return Err(format!("Failed to create UIView for type '{}'", view_type));
            }

            // Clip to bounds
            let _: () = msg_send![view, setClipsToBounds: true];

            log::info!(
                "IosPlatformView: created native UIView for type '{}' at ({}, {}, {}, {})",
                view_type,
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
            );

            Ok(view)
        }
    }

    /// Create a generic transparent UIView container.
    #[cfg(target_os = "ios")]
    unsafe fn create_generic_view(frame: ObjcCGRect) -> Result<*mut AnyObject, String> {
        let uiview_class = class!(UIView);
        let view: *mut AnyObject = msg_send![uiview_class, alloc];
        let view: *mut AnyObject = msg_send![view, initWithFrame: frame];
        if view.is_null() {
            return Err("Failed to create UIView".into());
        }
        let clear_color: *mut AnyObject = msg_send![class!(UIColor), clearColor];
        let _: () = msg_send![view, setBackgroundColor: clear_color];
        Ok(view)
    }

    /// Create a UIView with an AVPlayerLayer for video playback.
    #[cfg(target_os = "ios")]
    unsafe fn create_video_player_view(
        frame: ObjcCGRect,
        params: &PlatformViewParams,
    ) -> Result<*mut AnyObject, String> {
        let player_id = params
            .creation_params
            .get("player_id")
            .ok_or("video_player requires player_id")?
            .parse::<u32>()
            .map_err(|_| "video_player player_id must be an unsigned integer")?;
        // Hooke's helper acquires +1 while PLAYERS is locked. Retaining a raw
        // pointer after the old borrowed lookup would still race disposal.
        let player = crate::packages::video_player::ios_retain_player(player_id)
            .ok_or("Video player is disposed or missing")?;
        let player_layer: *mut AnyObject = msg_send![class!(AVPlayerLayer),
            playerLayerWithPlayer: objc2::rc::Retained::as_ptr(&player)];
        // AVPlayerLayer retains the player; the local acquisition is released
        // only after layer construction and container setup have completed.
        Self::create_video_layer_container(frame, player_layer)
    }

    unsafe fn create_video_layer_container(
        frame: ObjcCGRect,
        player_layer: *mut AnyObject,
    ) -> Result<*mut AnyObject, String> {
        if player_layer.is_null() {
            return Err("Failed to create AVPlayerLayer".into());
        }
        let view: *mut AnyObject = msg_send![class!(UIView), alloc];
        let view: *mut AnyObject = msg_send![view, initWithFrame: frame];
        let view = objc2::rc::Retained::from_raw(view)
            .ok_or("Failed to create UIView for video_player")?;
        let black_color: *mut AnyObject = msg_send![class!(UIColor), blackColor];
        let _: () = msg_send![&*view, setBackgroundColor: black_color];
        let view_layer: *mut AnyObject = msg_send![&*view, layer];
        if view_layer.is_null() {
            return Err("Video container has no backing layer".into());
        }
        let gravity = Self::make_nsstring("AVLayerVideoGravityResizeAspect");
        let _: () = msg_send![player_layer, setVideoGravity: gravity];
        let _: () = msg_send![gravity, release];
        let _: () = msg_send![view_layer, addSublayer: player_layer];
        Self::resize_video_layers(objc2::rc::Retained::as_ptr(&view).cast_mut());
        Ok(objc2::rc::Retained::into_raw(view))
    }

    unsafe fn resize_video_layers(view: *mut AnyObject) {
        let bounds: ObjcCGRect = msg_send![view, bounds];
        let local = ObjcCGRect::new(0.0, 0.0, bounds.width, bounds.height);
        let backing: *mut AnyObject = msg_send![view, layer];
        let layers: *mut AnyObject = msg_send![backing, sublayers];
        let count: usize = msg_send![layers, count];
        let _: () = msg_send![class!(CATransaction), begin];
        let _: () = msg_send![class!(CATransaction), setDisableActions: true];
        for index in 0..count {
            let layer: *mut AnyObject = msg_send![layers, objectAtIndex: index];
            let video: bool = msg_send![layer, isKindOfClass: class!(AVPlayerLayer)];
            if video {
                let _: () = msg_send![layer, setFrame: local];
            }
        }
        let _: () = msg_send![class!(CATransaction), commit];
    }

    /// Create a WKWebView.
    #[cfg(target_os = "ios")]
    unsafe fn create_webview_view(
        frame: ObjcCGRect,
        params: &PlatformViewParams,
    ) -> Result<*mut AnyObject, String> {
        if params
            .creation_params
            .get("dom_storage_enabled")
            .is_some_and(|v| v == "false")
        {
            return Err("WKWebView has no public per-view DOM storage disable setting".into());
        }
        let config: *mut AnyObject = msg_send![class!(WKWebViewConfiguration), alloc];
        let config: *mut AnyObject = msg_send![config, init];
        if config.is_null() {
            return Err("Failed to create WKWebViewConfiguration".into());
        }

        let js_enabled = params
            .creation_params
            .get("javascript_enabled")
            .map(|v| v == "true")
            .unwrap_or(true);

        let prefs: *mut AnyObject = msg_send![config, preferences];
        if !prefs.is_null() {
            let _: () = msg_send![prefs, setJavaScriptEnabled: js_enabled];
        }

        let webview: *mut AnyObject = msg_send![class!(WKWebView), alloc];
        let webview: *mut AnyObject =
            msg_send![webview, initWithFrame: frame, configuration: config];
        let _: () = msg_send![config, release];
        if webview.is_null() {
            return Err("Failed to create WKWebView".into());
        }

        if let Some(agent) = params.creation_params.get("user_agent") {
            let agent = Self::make_nsstring(agent);
            let _: () = msg_send![webview, setCustomUserAgent: agent];
            let _: () = msg_send![agent, release];
        }
        if params
            .creation_params
            .get("zoom_enabled")
            .is_some_and(|v| v == "false")
        {
            let scroll: *mut AnyObject = msg_send![webview, scrollView];
            let pinch: *mut AnyObject = msg_send![scroll, pinchGestureRecognizer];
            if !pinch.is_null() {
                let _: () = msg_send![pinch, setEnabled: false];
            }
        }

        // Load URL or HTML if provided
        if let Some(url) = params.creation_params.get("url") {
            if !url.is_empty() {
                let ns_url_str = Self::make_nsstring(url);
                let nsurl: *mut AnyObject = msg_send![class!(NSURL), URLWithString: ns_url_str];
                if nsurl.is_null() {
                    let _: () = msg_send![ns_url_str, release];
                    let _: () = msg_send![webview, release];
                    return Err("Invalid WebView URL".into());
                }
                if !nsurl.is_null() {
                    let request: *mut AnyObject =
                        msg_send![class!(NSURLRequest), requestWithURL: nsurl];
                    let _: *mut AnyObject = msg_send![webview, loadRequest: request];
                }
                let _: () = msg_send![ns_url_str, release];
            }
        } else if let Some(html) = params.creation_params.get("html") {
            if !html.is_empty() {
                let ns_html = Self::make_nsstring(html);
                let base_url: *mut AnyObject = std::ptr::null_mut();
                let _: *mut AnyObject =
                    msg_send![webview, loadHTMLString: ns_html, baseURL: base_url];
                let _: () = msg_send![ns_html, release];
            }
        }

        Ok(webview)
    }

    /// Create a UIView with AVCaptureVideoPreviewLayer for camera preview.
    #[cfg(target_os = "ios")]
    unsafe fn create_camera_preview_view(
        frame: ObjcCGRect,
        params: &PlatformViewParams,
    ) -> Result<*mut AnyObject, String> {
        let uiview_class = class!(UIView);
        let view: *mut AnyObject = msg_send![uiview_class, alloc];
        let view: *mut AnyObject = msg_send![view, initWithFrame: frame];
        if view.is_null() {
            return Err("Failed to create UIView for camera_preview".into());
        }

        let black_color: *mut AnyObject = msg_send![class!(UIColor), blackColor];
        let _: () = msg_send![view, setBackgroundColor: black_color];

        // If a session_id is provided, try to get the AVCaptureSession and create preview layer
        if let Some(session_id_str) = params.creation_params.get("session_id") {
            if let Ok(session_id) = session_id_str.parse::<usize>() {
                if let Some(session_ptr) = crate::packages::camera::ios_get_session(session_id) {
                    let layer: *mut AnyObject =
                        msg_send![class!(AVCaptureVideoPreviewLayer), alloc];
                    let layer: *mut AnyObject = msg_send![layer, initWithSession: session_ptr];
                    if !layer.is_null() {
                        let _: () = msg_send![layer, setFrame: frame];
                        let gravity = Self::make_nsstring("AVLayerVideoGravityResizeAspectFill");
                        let _: () = msg_send![layer, setVideoGravity: gravity];
                        let _: () = msg_send![gravity, release];
                        let view_layer: *mut AnyObject = msg_send![view, layer];
                        let _: () = msg_send![view_layer, addSublayer: layer];
                    }
                }
            }
        }

        Ok(view)
    }

    #[cfg(target_os = "ios")]
    unsafe fn make_nsstring(s: &str) -> *mut AnyObject {
        let string = crate::ios::util::nsstring(s);
        msg_send![string, retain]
    }

    /// Returns the raw pointer to the underlying `UIView`.
    ///
    /// Use this to insert the view into the iOS view hierarchy from
    /// the window/paint code. Returns null if the view has been disposed.
    #[cfg(target_os = "ios")]
    pub fn native_view_ptr(&self) -> *mut AnyObject {
        *self.native_view.lock().unwrap()
    }

    /// Insert this view into the GPUI window's view hierarchy.
    ///
    /// Adds the UIView as a subview of the Metal view's superview (the
    /// view controller's view), above the Metal surface for native rendering
    /// and hit testing. Native controls cover overlapping GPUI content.
    ///
    /// Called with the actual painting window; also supports reparenting.
    #[cfg(target_os = "ios")]
    pub fn insert_into_window(&self, window: &gpui::Window) -> Result<(), String> {
        attach_to_paint_window(self.id, window)
    }

    /// Update the native view's frame.
    #[cfg(target_os = "ios")]
    fn update_native_frame(&self, bounds: &PlatformViewBounds) {
        on_main(|| {
            let view = *self.native_view.lock().unwrap();
            if view.is_null() {
                return;
            }
            unsafe {
                let frame = ObjcCGRect::new(
                    bounds.x as f64,
                    bounds.y as f64,
                    bounds.width as f64,
                    bounds.height as f64,
                );
                let _: () = msg_send![view, setFrame: frame];
                if self.view_type == "video_player" {
                    Self::resize_video_layers(view);
                }
            }
        });
    }
}

impl PlatformView for IosPlatformView {
    fn id(&self) -> PlatformViewId {
        self.id
    }

    fn view_type(&self) -> &str {
        &self.view_type
    }

    fn set_bounds(&self, bounds: PlatformViewBounds) {
        if self.disposed.load(Ordering::Relaxed) {
            return;
        }
        *self.bounds.lock().unwrap() = bounds;
        #[cfg(target_os = "ios")]
        {
            self.update_native_frame(&bounds);
        }
    }

    fn set_visible(&self, visible: bool) {
        if self.disposed.load(Ordering::Relaxed) {
            return;
        }
        #[cfg(target_os = "ios")]
        {
            on_main(|| {
                let view = *self.native_view.lock().unwrap();
                if !view.is_null() {
                    unsafe {
                        let _: () = msg_send![view, setHidden: !visible];
                    }
                }
            });
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = visible;
        }
    }

    fn set_z_index(&self, z_index: i32) {
        if self.disposed.load(Ordering::Relaxed) {
            return;
        }
        #[cfg(target_os = "ios")]
        {
            on_main(|| {
                let view = *self.native_view.lock().unwrap();
                if !view.is_null() {
                    unsafe {
                        let layer: *mut AnyObject = msg_send![view, layer];
                        if !layer.is_null() {
                            let z = z_index as f64;
                            let _: () = msg_send![layer, setZPosition: z];
                        }
                    }
                }
            });
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = z_index;
        }
    }

    fn dispose(&self) {
        if self.disposed.swap(true, Ordering::Relaxed) {
            return;
        }
        #[cfg(target_os = "ios")]
        {
            on_main(|| {
                native_views().lock().unwrap().remove(&self.id);
                let mut pointer = self.native_view.lock().unwrap();
                let view = std::mem::replace(&mut *pointer, std::ptr::null_mut());
                if !view.is_null() {
                    unsafe {
                        if self.view_type == "webview" {
                            let _: () = msg_send![view, stopLoading];
                        }
                        let _: () = msg_send![view, removeFromSuperview];
                        let _: () = msg_send![view, release];
                    }
                }
            });
        }
        log::info!("IosPlatformView: disposed view {}", self.id);
    }

    fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::Relaxed)
    }
}

/// iOS platform view factory.
pub struct IosPlatformViewFactory {
    view_type: String,
}

impl IosPlatformViewFactory {
    pub fn new(view_type: &str) -> Self {
        Self {
            view_type: view_type.to_string(),
        }
    }
}

impl PlatformViewFactory for IosPlatformViewFactory {
    fn create(&self, params: &PlatformViewParams) -> Result<Box<dyn PlatformView>, String> {
        #[cfg(target_os = "ios")]
        {
            let view = IosPlatformView::new(&self.view_type, params)?;
            Ok(Box::new(view))
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = params;
            Err("iOS platform views are only available on iOS".to_string())
        }
    }

    fn view_type(&self) -> &str {
        &self.view_type
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;

    #[test]
    fn video_creation_rejects_missing_invalid_and_absent_players_and_nil_layer() {
        on_main(|| unsafe {
            let frame = ObjcCGRect::new(40.0, 70.0, 120.0, 90.0);
            let mut params = PlatformViewParams::default();
            assert!(IosPlatformView::create_video_player_view(frame, &params)
                .unwrap_err()
                .contains("requires player_id"));
            params
                .creation_params
                .insert("player_id".into(), "invalid".into());
            assert!(IosPlatformView::create_video_player_view(frame, &params)
                .unwrap_err()
                .contains("unsigned integer"));
            // Package IDs start at one; zero never identifies a live test player.
            params
                .creation_params
                .insert("player_id".into(), "0".into());
            assert!(IosPlatformView::create_video_player_view(frame, &params)
                .unwrap_err()
                .contains("disposed or missing"));
            assert!(
                IosPlatformView::create_video_layer_container(frame, std::ptr::null_mut())
                    .unwrap_err()
                    .contains("AVPlayerLayer")
            );
        });
    }

    #[test]
    fn video_layer_retains_player_and_tracks_local_bounds() {
        on_main(|| {
            objc2::rc::autoreleasepool(|_| unsafe {
                let raw: *mut AnyObject = msg_send![class!(AVQueuePlayer), new];
                let player = objc2::rc::Retained::from_raw(raw).unwrap();
                let layer: *mut AnyObject = msg_send![class!(AVPlayerLayer),
                playerLayerWithPlayer: objc2::rc::Retained::as_ptr(&player)];
                assert!(!layer.is_null());
                let frame = ObjcCGRect::new(40.0, 70.0, 120.0, 90.0);
                let view = objc2::rc::Retained::from_raw(
                    IosPlatformView::create_video_layer_container(frame, layer).unwrap(),
                )
                .unwrap();
                drop(player);
                let retained_player: *mut AnyObject = msg_send![layer, player];
                assert_eq!(retained_player, raw);
                let _: f32 = msg_send![retained_player, rate];
                let bounds: ObjcCGRect = msg_send![layer, frame];
                assert_eq!(
                    (bounds.x, bounds.y, bounds.width, bounds.height),
                    (0.0, 0.0, 120.0, 90.0)
                );
                let _: () = msg_send![&*view, setFrame: ObjcCGRect::new(80.0, 50.0, 300.0, 200.0)];
                IosPlatformView::resize_video_layers(objc2::rc::Retained::as_ptr(&view).cast_mut());
                let bounds: ObjcCGRect = msg_send![layer, frame];
                assert_eq!(
                    (bounds.x, bounds.y, bounds.width, bounds.height),
                    (0.0, 0.0, 300.0, 200.0)
                );
                // Owned container drops here; autoreleased layer/player ownership drains with the pool.
            })
        });
    }

    #[test]
    fn attachment_uses_paint_window_and_can_move_between_windows() {
        on_main(|| unsafe {
            struct Owned(Vec<*mut AnyObject>);
            impl Drop for Owned {
                fn drop(&mut self) {
                    for object in self.0.iter().rev() {
                        unsafe {
                            let _: () = msg_send![*object, release];
                        }
                    }
                }
            }
            let mut owned = Owned(Vec::new());
            let a: *mut AnyObject = msg_send![class!(UIWindow), new];
            owned.0.push(a);
            let b: *mut AnyObject = msg_send![class!(UIWindow), new];
            owned.0.push(b);
            let metal_a: *mut AnyObject = msg_send![class!(UIView), new];
            owned.0.push(metal_a);
            let metal_b: *mut AnyObject = msg_send![class!(UIView), new];
            owned.0.push(metal_b);
            let embedded: *mut AnyObject = msg_send![class!(UIView), new];
            owned.0.push(embedded);
            let _: () = msg_send![a, addSubview: metal_a];
            let _: () = msg_send![b, addSubview: metal_b];

            // B was allocated later, but A paints this element first.
            attach_above_metal(embedded, metal_a).unwrap();
            let parent: *mut AnyObject = msg_send![embedded, superview];
            assert_eq!(parent, a);
            attach_above_metal(embedded, metal_a).unwrap();
            attach_above_metal(embedded, metal_b).unwrap();
            let parent: *mut AnyObject = msg_send![embedded, superview];
            assert_eq!(parent, b);
            let _: () = msg_send![metal_b, removeFromSuperview];
            assert!(attach_above_metal(embedded, metal_b).is_err());
        });
    }
}
