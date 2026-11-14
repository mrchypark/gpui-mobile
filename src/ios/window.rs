//! iOS Window implementation using UIWindow and UIViewController.
//!
//! iOS windows are fundamentally different from desktop windows:
//! - Always fullscreen (or split-screen on iPad)
//! - No title bar or window chrome
//! - Touch-based input
//! - Safe area insets for notch/home indicator
//!
//! The window is backed by a UIWindow containing a UIViewController
//! whose view hosts a CAMetalLayer. Rendering is performed by
//! `gpui_wgpu::WgpuRenderer` which drives wgpu over the Metal backend.

use super::IosDisplay;
#[path = "input_traits.rs"]
mod input_traits;
use super::events::*;
use crate::mobile_dispatch::Callback;
use crate::mobile_events::{
    self, AppLifecycle, MobileEvent, MobileEventState, SurfaceLifecycle, WindowInsets,
};
use crate::momentum::{MomentumScroller, VelocityTracker};
use gpui::{
    point, px, size, AnyWindowHandle, AtlasKey, AtlasTextureId, AtlasTextureKind, AtlasTile,
    Bounds, Capslock, DevicePixels, DispatchEventResult, GpuSpecs, Modifiers, Pixels,
    PlatformAtlas, PlatformDisplay, PlatformInput, PlatformInputHandler, PlatformWindow, Point,
    PromptButton, PromptLevel, RequestFrameOptions, Scene, Size, TileId, WindowAppearance,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowParams,
};
use gpui_wgpu::{GpuContext, WgpuContext, WgpuRenderer, WgpuSurfaceConfig};
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};

use super::cg_types::ObjcCGRect;
use parking_lot::Mutex;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, UiKitDisplayHandle, UiKitWindowHandle};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::c_void,
    ptr::{self, NonNull},
    rc::Rc,
    sync::Arc,
};

const GPUI_WINDOW_IVAR: &str = "gpui_window_ptr";
const IME_NATIVE_EDIT_IVAR: &str = "gpui_ime_native_edit";

// UIKit may call another overridden input method while updating its document.
// Only the outer native operation delivers to the GPUI document.
#[allow(deprecated)]
unsafe fn native_ime_edit(this: *mut AnyObject, edit: impl FnOnce()) {
    let previous = *(*this).get_ivar::<Bool>(IME_NATIVE_EDIT_IVAR);
    *(*this).get_mut_ivar::<Bool>(IME_NATIVE_EDIT_IVAR) = Bool::YES;
    edit();
    *(*this).get_mut_ivar::<Bool>(IME_NATIVE_EDIT_IVAR) = previous;
}

#[allow(deprecated)]
unsafe fn inside_native_ime_edit(this: *mut AnyObject) -> bool {
    (*(*this).get_ivar::<Bool>(IME_NATIVE_EDIT_IVAR)).as_bool()
}

/// Identifies the logical text field currently represented by the one UIKit
/// proxy. UIKit retains that proxy while GPUI replaces input handlers during
/// painting, so only a changed focus id starts a new proxy generation.
struct ProxyInputSession<F> {
    focus: Option<F>,
    generation: u64,
}

impl<F: PartialEq> ProxyInputSession<F> {
    fn new() -> Self {
        Self {
            focus: None,
            generation: 0,
        }
    }

    /// Returns true when a different text field now owns the UIKit proxy.
    fn set_focus(&mut self, focus: Option<F>) -> bool {
        if self.focus.as_ref() == focus.as_ref() {
            return false;
        }
        self.focus = focus;
        self.generation = self.generation.wrapping_add(1);
        true
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }
}

unsafe fn clear_proxy_context(this: *mut AnyObject, window: &IosWindowInner) {
    native_ime_edit(this, || {
        let _: () = msg_send![this, setText: ptr::null_mut::<AnyObject>()];
    });
    *window.proxy_context.borrow_mut() = None;
    window.proxy_base.set(None);
    window.proxy_mapping_valid.set(false);
    window.proxy_text.borrow_mut().clear();
}

unsafe fn sync_proxy_context(this: *mut AnyObject, window: &IosWindowInner, before_edit: bool) {
    let native: *mut AnyObject = msg_send![this, text];
    let utf8: *const i8 = msg_send![native, UTF8String];
    let native = if utf8.is_null() {
        String::new()
    } else {
        std::ffi::CStr::from_ptr(utf8)
            .to_string_lossy()
            .into_owned()
    };
    // UIKit can mutate textStorage directly before invoking insertText. Verify
    // the last acknowledged document, not this partially applied native edit.
    let expected = if before_edit {
        window.proxy_text.borrow().clone()
    } else {
        native.clone()
    };
    let (context, matches) = window
        .input_handler
        .call(
            || !window.closed.get(),
            |handler| {
                let (accepts, owner) =
                    mobile_events::query_text_input_owner(|| handler.query_accepts_text_input());
                if !accepts {
                    return (None, false);
                }
                let context = handler
                    .selected_text_range(false)
                    .map(|selection| (owner, selection.range));
                let matches = window
                    .proxy_base
                    .get()
                    .and_then(|base| {
                        let end = base.checked_add(expected.encode_utf16().count())?;
                        let range = base..end;
                        let mut adjusted = None;
                        let actual = handler.text_for_range(range.clone(), &mut adjusted)?;
                        Some(
                            actual == expected && adjusted.is_none_or(|adjusted| adjusted == range),
                        )
                    })
                    .unwrap_or(false);
                (context, matches)
            },
        )
        .unwrap_or((None, false));
    let changed = *window.proxy_context.borrow() != context;
    let owner_changed =
        window.proxy_context.borrow().as_ref().map(|c| c.0) != context.as_ref().map(|c| c.0);
    if (before_edit && changed) || owner_changed || !matches {
        clear_proxy_context(this, window);
        window
            .proxy_base
            .set(context.as_ref().map(|(_, range)| range.start));
    } else if !before_edit {
        // A void handler callback is not acceptance. Only authoritative text
        // at the original anchor can certify this native document's mapping.
        window.proxy_mapping_valid.set(true);
        *window.proxy_text.borrow_mut() = native;
    }
    *window.proxy_context.borrow_mut() = context;
}

// Describe one whole native edit, including textStorage mutations that precede
// an overridden callback. Offsets are UTF-16; comparison boundaries are scalars.
fn native_text_edit<'a>(before: &str, after: &'a str) -> (std::ops::Range<usize>, &'a str) {
    let prefix = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    let suffix = before[prefix..]
        .chars()
        .rev()
        .zip(after[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    let start = before[..prefix].encode_utf16().count();
    let end = before[..before.len() - suffix].encode_utf16().count();
    (start..end, &after[prefix..after.len() - suffix])
}

fn appearance_style_changed(previous: Option<i64>, current: Option<i64>) -> bool {
    matches!((previous, current), (Some(previous), Some(current)) if previous != current)
}

fn edge_attached_keyboard_insets(
    window: Bounds<Pixels>,
    keyboard: Option<Bounds<Pixels>>,
) -> gpui::Edges<Pixels> {
    let Some(keyboard) = keyboard else {
        return Default::default();
    };
    let spans_width = keyboard.left() <= window.left() && keyboard.right() >= window.right();
    let spans_height = keyboard.top() <= window.top() && keyboard.bottom() >= window.bottom();

    if spans_width && keyboard.bottom() >= window.bottom() {
        gpui::Edges {
            bottom: window.bottom() - keyboard.top(),
            ..Default::default()
        }
    } else if spans_width && keyboard.top() <= window.top() {
        gpui::Edges {
            top: keyboard.bottom() - window.top(),
            ..Default::default()
        }
    } else if spans_height && keyboard.right() >= window.right() {
        gpui::Edges {
            right: window.right() - keyboard.left(),
            ..Default::default()
        }
    } else if spans_height && keyboard.left() <= window.left() {
        gpui::Edges {
            left: keyboard.right() - window.left(),
            ..Default::default()
        }
    } else {
        Default::default()
    }
}

fn platform_insets(window: Bounds<Pixels>, insets: &WindowInsets) -> gpui::WindowInsets {
    gpui::WindowInsets {
        safe_area: insets.safe_area,
        ime: insets
            .keyboard_insets
            .unwrap_or_else(|| edge_attached_keyboard_insets(window, insets.keyboard)),
        ..Default::default()
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ImeRange {
    location: usize,
    length: usize,
}
unsafe impl Encode for ImeRange {
    const ENCODING: Encoding = Encoding::Struct("_NSRange", &[usize::ENCODING, usize::ENCODING]);
}

/// Lightweight window handle for wgpu surface creation.
/// Stores the raw UIView pointer needed by wgpu to create a Metal surface.
/// Implements the traits required by `WgpuRenderer::new`.
#[derive(Debug, Clone, Copy)]
struct RawIosWindow {
    view: *mut c_void,
}

unsafe impl Send for RawIosWindow {}
unsafe impl Sync for RawIosWindow {}

impl HasWindowHandle for RawIosWindow {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
    {
        let view = NonNull::new(self.view).ok_or(raw_window_handle::HandleError::Unavailable)?;
        let handle = UiKitWindowHandle::new(view);
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(handle.into()) })
    }
}

impl HasDisplayHandle for RawIosWindow {
    fn display_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError>
    {
        let handle = UiKitDisplayHandle::new();
        Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(handle.into()) })
    }
}

static METAL_VIEW_CLASS_REGISTERED: std::sync::Once = std::sync::Once::new();
static VC_CLASS_REGISTERED: std::sync::Once = std::sync::Once::new();
static TEXT_INPUT_VIEW_CLASS_REGISTERED: std::sync::Once = std::sync::Once::new();

/// Global storage for the current status bar style.
/// 0 = default (dark content), 1 = light content.
/// Accessed from the main thread only.
static STATUS_BAR_STYLE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Register a custom UIViewController subclass that allows overriding
/// `preferredStatusBarStyle` at runtime.
fn register_view_controller_class() -> &'static AnyClass {
    VC_CLASS_REGISTERED.call_once(|| {
        let superclass = class!(UIViewController);
        let mut decl = ClassBuilder::new(c"GPUIViewController", superclass).unwrap();

        // Override preferredStatusBarStyle
        extern "C" fn preferred_status_bar_style(_this: *mut AnyObject, _sel: Sel) -> isize {
            let style = STATUS_BAR_STYLE.load(std::sync::atomic::Ordering::Relaxed);
            if style == 1 {
                1 // UIStatusBarStyleLightContent
            } else {
                3 // UIStatusBarStyleDarkContent (iOS 13+)
            }
        }

        // Override viewDidLayoutSubviews — called by UIKit on rotation,
        // split-screen changes, and any other layout pass.
        extern "C" fn view_did_layout_subviews(this: *mut AnyObject, _sel: Sel) {
            // Call super
            unsafe {
                let superclass = class!(UIViewController);
                let _: () = msg_send![super(this, superclass), viewDidLayoutSubviews];
            }

            // Notify all registered GPUI windows about the layout change.
            for window in super::ffi::windows() {
                if !window.closed.get() {
                    window.handle_layout_change();
                }
            }
        }

        unsafe {
            decl.add_method(
                sel!(preferredStatusBarStyle),
                preferred_status_bar_style as extern "C" fn(*mut AnyObject, Sel) -> isize,
            );
            decl.add_method(
                sel!(viewDidLayoutSubviews),
                view_did_layout_subviews as extern "C" fn(*mut AnyObject, Sel),
            );
        }

        decl.register();
    });

    class!(GPUIViewController)
}

/// Set the iOS status bar content style (light or dark text/icons).
///
/// This updates the stored style and asks the root view controller
/// to re-query `preferredStatusBarStyle`.
pub fn set_status_bar_style(style: crate::StatusBarContentStyle) {
    use crate::StatusBarContentStyle;

    let value = match style {
        StatusBarContentStyle::Light => 1,
        StatusBarContentStyle::Dark => 0,
    };
    STATUS_BAR_STYLE.store(value, std::sync::atomic::Ordering::Relaxed);

    // Ask UIKit to re-query the status bar style
    unsafe {
        if let Some(window) = super::ffi::windows().last() {
            let vc = window.view_controller;
            if !vc.is_null() {
                let _: () = msg_send![vc, setNeedsStatusBarAppearanceUpdate];
            }
        }
    }
}

/// Register a custom UIView subclass that uses CAMetalLayer as its backing layer.
/// This is required for Metal rendering on iOS.
fn register_metal_view_class() -> &'static AnyClass {
    METAL_VIEW_CLASS_REGISTERED.call_once(|| {
        let superclass = class!(UIView);
        let mut decl = ClassBuilder::new(c"GPUIMetalView", superclass).unwrap();

        // Add ivar to store window pointer for touch handling
        decl.add_ivar::<*mut std::ffi::c_void>(c"gpui_window_ptr");

        // Override layerClass to return CAMetalLayer
        extern "C" fn layer_class(_self: *const AnyClass, _sel: Sel) -> *const AnyClass {
            class!(CAMetalLayer) as *const AnyClass
        }

        // Touch handling methods
        extern "C" fn touches_began(
            this: *mut AnyObject,
            _sel: Sel,
            touches: *mut AnyObject,
            event: *mut AnyObject,
        ) {
            handle_touches(this, touches, event);
        }

        extern "C" fn touches_moved(
            this: *mut AnyObject,
            _sel: Sel,
            touches: *mut AnyObject,
            event: *mut AnyObject,
        ) {
            handle_touches(this, touches, event);
        }

        extern "C" fn touches_ended(
            this: *mut AnyObject,
            _sel: Sel,
            touches: *mut AnyObject,
            event: *mut AnyObject,
        ) {
            handle_touches(this, touches, event);
        }

        extern "C" fn touches_cancelled(
            this: *mut AnyObject,
            _sel: Sel,
            touches: *mut AnyObject,
            event: *mut AnyObject,
        ) {
            handle_touches(this, touches, event);
        }

        // UIKit reports system appearance changes on the view that owns the
        // dynamic colors. Delegate to the window that created this view.
        unsafe extern "C" fn trait_collection_did_change(
            this: *mut AnyObject,
            _sel: Sel,
            previous: *mut AnyObject,
        ) {
            let _: () = msg_send![super(this, class!(UIView)),
                traitCollectionDidChange: previous];

            let current: *mut AnyObject = msg_send![this, traitCollection];
            let previous_style =
                (!previous.is_null()).then(|| msg_send![previous, userInterfaceStyle]);
            let current_style =
                (!current.is_null()).then(|| msg_send![current, userInterfaceStyle]);
            if !appearance_style_changed(previous_style, current_style) {
                return;
            }

            #[allow(deprecated)]
            let window_ptr: *mut c_void = *(*this).get_ivar(GPUI_WINDOW_IVAR);
            let Some(window) = super::ffi::window_for_handle(window_ptr as usize) else {
                return;
            };
            let delivered = window
                .appearance_changed_callback
                .call(|| !window.closed.get(), |callback| callback())
                .is_some();
            log::info!(
                "GPUI iOS: Appearance changed - previous: {:?}, current: {:?}, delivered: {}",
                previous_style,
                current_style,
                delivered
            );
        }

        unsafe {
            // Add class method for layerClass
            decl.add_class_method(
                sel!(layerClass),
                layer_class as extern "C" fn(*const AnyClass, Sel) -> *const AnyClass,
            );

            // Add touch handling instance methods
            decl.add_method(
                sel!(touchesBegan:withEvent:),
                touches_began as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            );
            decl.add_method(
                sel!(touchesMoved:withEvent:),
                touches_moved as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            );
            decl.add_method(
                sel!(touchesEnded:withEvent:),
                touches_ended as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            );
            decl.add_method(
                sel!(touchesCancelled:withEvent:),
                touches_cancelled
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            );
            decl.add_method(
                sel!(traitCollectionDidChange:),
                trait_collection_did_change
                    as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            unsafe extern "C" fn pinch(
                this: *mut AnyObject,
                _sel: Sel,
                recognizer: *mut AnyObject,
            ) {
                #[allow(deprecated)]
                let window: *mut c_void = *(*this).get_ivar(GPUI_WINDOW_IVAR);
                if window.is_null() {
                    return;
                }
                let Some(window) = super::ffi::window_for_handle(window as usize) else {
                    return;
                };
                let state: isize = msg_send![recognizer, state];
                let phase = match state {
                    1 => gpui::TouchPhase::Started,
                    2 => gpui::TouchPhase::Moved,
                    3 => gpui::TouchPhase::Ended,
                    4 | 5 => gpui::TouchPhase::Cancelled,
                    _ => return,
                };
                let center: super::cg_types::ObjcCGPoint =
                    msg_send![recognizer, locationInView: this];
                let scale: f64 = msg_send![recognizer, scale];
                let center_pt = point(px(center.x as f32), px(center.y as f32));
                let scale_f32 = scale as f32;
                let event = window.mobile_events.borrow_mut().pinch(
                    phase,
                    center_pt,
                    scale_f32,
                    std::time::Duration::ZERO,
                );
                if let Some(event) = event {
                    window.emit_mobile_events(vec![event]);
                }
                if !crate::mobile_events::raw_touch_only() && !window.closed.get() {
                    if let Some(delta) =
                        step_pinch_delta(&window.last_pinch_scale, phase, scale_f32)
                    {
                        window.dispatch_input(PlatformInput::Pinch(gpui::PinchEvent {
                            position: center_pt,
                            delta,
                            modifiers: window.modifiers.get(),
                            phase,
                        }));
                    }
                }
            }
            decl.add_method(
                sel!(gpuiPinch:),
                pinch as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            unsafe extern "C" fn did_move_to_window(this: *mut AnyObject, _sel: Sel) {
                let _: () = msg_send![super(this, class!(UIView)), didMoveToWindow];
                #[allow(deprecated)]
                let window: *mut c_void = *(*this).get_ivar(GPUI_WINDOW_IVAR);
                if let Some(window) = super::ffi::window_for_handle(window as usize) {
                    window.sync_surface_availability();
                }
            }
            decl.add_method(
                sel!(didMoveToWindow),
                did_move_to_window as unsafe extern "C" fn(*mut AnyObject, Sel),
            );
        }

        decl.register();
    });

    class!(GPUIMetalView)
}

/// UIKit owns the insertion proxy's composition storage and UITextInput protocol;
/// GPUI owns the document. The legacy string callback receives committed text only.
fn register_text_input_view_class() -> &'static AnyClass {
    TEXT_INPUT_VIEW_CLASS_REGISTERED.call_once(|| {
        // UITextView supplies UIKit's positions, ranges, tokenizer and composing
        // storage. GPUI remains the document owner; this is an insertion proxy.
        let superclass = class!(UITextView);
        let mut decl = ClassBuilder::new(c"GPUITextInputView", superclass).unwrap();
        input_traits::register(&mut decl);
        if let Some(protocol) = objc2::runtime::AnyProtocol::get(c"UITextInputTraits") {
            decl.add_protocol(protocol);
        }

        // Declare protocol conformance so iOS knows this view can receive
        // keyboard text input.
        if let Some(protocol) = objc2::runtime::AnyProtocol::get(c"UIKeyInput") {
            decl.add_protocol(protocol);
        }

        // Store the IosWindow pointer so callbacks can reach the Rust window.
        decl.add_ivar::<*mut std::ffi::c_void>(c"gpui_window_ptr");
        decl.add_ivar::<Bool>(c"gpui_ime_native_edit");

        // --- UIKeyInput protocol methods ---

        // Bool hasText
        unsafe extern "C" fn has_text(_this: *mut AnyObject, _sel: Sel) -> Bool {
            Bool::YES
        }

        // void insertText:(NSString *)text
        unsafe extern "C" fn insert_text(this: *mut AnyObject, _sel: Sel, text: *mut AnyObject) {
            if inside_native_ime_edit(this) {
                let _: () = msg_send![super(this, class!(UITextView)), insertText: text];
                return;
            }
            #[allow(deprecated)]
            let window_ptr: *mut std::ffi::c_void = *(*this).get_ivar(GPUI_WINDOW_IVAR);
            if window_ptr.is_null() || text.is_null() {
                return;
            }
            let Some(window) = super::ffi::window_for_handle(window_ptr as usize) else {
                return;
            };
            let generation = window.proxy_generation();
            let text: *mut AnyObject = msg_send![text, retain];
            sync_proxy_context(this, &window, true);
            let acknowledged = window.proxy_text.borrow().clone();
            let mapped_base = window.proxy_base.get().filter(|_| window.proxy_mapping_valid.get());
            // Korean keyboards replace earlier syllable fragments using the
            // native document. Clearing it turns each fragment into an append.
            native_ime_edit(this, || {
                let _: () = msg_send![super(this, class!(UITextView)), insertText: text];
            });
            let delivered = if !crate::mobile_dispatch::has_text_handler() {
                let utf8: *const i8 = msg_send![text, UTF8String];
                if utf8.is_null() { false } else {
                    let value = std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned();
                    let native: *mut AnyObject = msg_send![this, text];
                    let native_utf8: *const i8 = msg_send![native, UTF8String];
                    let current = if native_utf8.is_null() { String::new() } else {
                        std::ffi::CStr::from_ptr(native_utf8).to_string_lossy().into_owned()
                    };
                    window.input_handler.call(|| !window.closed.get(), |handler| {
                        if let Some(base) = mapped_base {
                            let (range, replacement) = native_text_edit(&acknowledged, &current);
                            if let (Some(start), Some(end)) = (base.checked_add(range.start), base.checked_add(range.end)) {
                                handler.replace_text_in_range(Some(start..end), replacement);
                                // An unchanged-text commit still ends composition.
                                // Some handlers ignore an empty textual diff.
                                if handler.marked_text_range().is_some() {
                                    handler.unmark_text();
                                }
                            }
                        } else {
                            handler.replace_text_in_range(None, &value);
                        }
                    }).is_some()
                }
            } else { false };
            if window.proxy_generation_is_current(generation) && !delivered {
                window.handle_text_input(text);
            }
            if window.proxy_generation_is_current(generation) {
                sync_proxy_context(this, &window, false);
            }
            let _: () = msg_send![text, release];
        }

        unsafe fn update_marked_text(
            this: *mut AnyObject,
            text: *mut AnyObject,
            selection: ImeRange,
            apply_native: impl FnOnce(),
        ) {
            if inside_native_ime_edit(this) {
                apply_native();
                return;
            }
            #[allow(deprecated)]
            let handle = *(*this).get_ivar::<*mut c_void>(GPUI_WINDOW_IVAR);
            let Some(window) = super::ffi::window_for_handle(handle as usize) else { return; };
            let generation = window.proxy_generation();
            sync_proxy_context(this, &window, true);
            native_ime_edit(this, apply_native);
            let text = if text.is_null() { String::new() } else {
                let utf8: *const i8 = msg_send![text, UTF8String];
                if utf8.is_null() { return; }
                std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
            };
            let selection = crate::mobile_dispatch::composing_selection(&text, selection.location, selection.length);
            if !crate::mobile_dispatch::has_text_handler() { window.input_handler.call(|| !window.closed.get(), |handler| {
                handler.replace_and_mark_text_in_range(None, &text, Some(selection));
            }); }
            if window.proxy_generation_is_current(generation) {
                sync_proxy_context(this, &window, false);
            }
        }

        unsafe extern "C" fn set_marked_text(this: *mut AnyObject, _sel: Sel, text: *mut AnyObject, selection: ImeRange) {
            update_marked_text(this, text, selection, || {
                let _: () = msg_send![super(this, class!(UITextView)), setMarkedText: text, selectedRange: selection];
            });
        }

        unsafe extern "C" fn set_attributed_marked_text(this: *mut AnyObject, _sel: Sel, text: *mut AnyObject, selection: ImeRange) {
            let plain: *mut AnyObject = msg_send![text, string];
            update_marked_text(this, plain, selection, || {
                let _: () = msg_send![super(this, class!(UITextView)), setAttributedMarkedText: text, selectedRange: selection];
            });
        }

        unsafe extern "C" fn replace_range(this: *mut AnyObject, _sel: Sel, range: *mut AnyObject, text: *mut AnyObject) {
            if inside_native_ime_edit(this) {
                let _: () = msg_send![super(this, class!(UITextView)), replaceRange: range, withText: text];
                return;
            }
            #[allow(deprecated)]
            let handle = *(*this).get_ivar::<*mut c_void>(GPUI_WINDOW_IVAR);
            let Some(window) = super::ffi::window_for_handle(handle as usize) else { return; };
            if range.is_null() || text.is_null() { return; }
            let generation = window.proxy_generation();
            // Do not apply a range obtained from a previous owner's document.
            let previous = window.proxy_context.borrow().clone();
            sync_proxy_context(this, &window, true);
            if !window.proxy_generation_is_current(generation)
                || previous != *window.proxy_context.borrow()
                || !window.proxy_mapping_valid.get()
            {
                return;
            }
            let beginning: *mut AnyObject = msg_send![this, beginningOfDocument];
            let start: *mut AnyObject = msg_send![range, start];
            let end: *mut AnyObject = msg_send![range, end];
            let start: isize = msg_send![this, offsetFromPosition: beginning, toPosition: start];
            let end: isize = msg_send![this, offsetFromPosition: beginning, toPosition: end];
            let utf8: *const i8 = msg_send![text, UTF8String];
            if utf8.is_null() || start < 0 || end < start { return; }
            let replacement = std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned();
            // The proxy contains the current insertion context, not the full
            // document. Translate from its caret to the actual owner's caret.
            window.input_handler.call(|| !window.closed.get(), |handler| {
                let Some(base) = window.proxy_base.get() else { return; };
                let Some(owner_start) = base.checked_add(start as usize) else { return; };
                let Some(owner_end) = base.checked_add(end as usize) else { return; };
                native_ime_edit(this, || {
                    let _: () = msg_send![super(this, class!(UITextView)), replaceRange: range, withText: text];
                });
                handler.replace_text_in_range(Some(owner_start..owner_end), &replacement);
            });
            if window.proxy_generation_is_current(generation) {
                sync_proxy_context(this, &window, false);
            }
        }

        unsafe extern "C" fn unmark_text(this: *mut AnyObject, _sel: Sel) {
            if inside_native_ime_edit(this) {
                let _: () = msg_send![super(this, class!(UITextView)), unmarkText];
                return;
            }
            #[allow(deprecated)]
            let handle = *(*this).get_ivar::<*mut c_void>(GPUI_WINDOW_IVAR);
            let Some(window) = super::ffi::window_for_handle(handle as usize) else { return; };
            let generation = window.proxy_generation();
            sync_proxy_context(this, &window, true);
            let marked: *mut AnyObject = msg_send![this, markedTextRange];
            if marked.is_null() { return; }
            let text: *mut AnyObject = msg_send![this, textInRange: marked];
            let text: *mut AnyObject = msg_send![text, retain];
            native_ime_edit(this, || {
                let _: () = msg_send![super(this, class!(UITextView)), unmarkText];
            });
            let should_fallback = crate::mobile_dispatch::has_text_handler()
                || window
                    .input_handler
                    .call(|| !window.closed.get(), |handler| handler.unmark_text())
                    .is_none();
            if window.proxy_generation_is_current(generation) && should_fallback {
                window.handle_text_input(text);
            }
            let _: () = msg_send![text, release];
            if window.proxy_generation_is_current(generation) {
                sync_proxy_context(this, &window, false);
            }
        }

        // void deleteBackward
        unsafe extern "C" fn delete_backward(this: *mut AnyObject, _sel: Sel) {
            if inside_native_ime_edit(this) {
                let _: () = msg_send![super(this, class!(UITextView)), deleteBackward];
                return;
            }
            #[allow(deprecated)]
            let window_ptr: *mut std::ffi::c_void = *(*this).get_ivar(GPUI_WINDOW_IVAR);
            if window_ptr.is_null() {
                return;
            }
            let Some(window) = super::ffi::window_for_handle(window_ptr as usize) else {
                return;
            };
            let generation = window.proxy_generation();
            sync_proxy_context(this, &window, true);
            let marked: *mut AnyObject = msg_send![this, markedTextRange];
            if !marked.is_null() {
                let beginning: *mut AnyObject = msg_send![this, beginningOfDocument];
                let start: *mut AnyObject = msg_send![marked, start];
                let start: isize = msg_send![this, offsetFromPosition: beginning, toPosition: start];
                if start < 0 { return; }
                let text: *mut AnyObject = msg_send![this, textInRange: marked];
                let length: usize = msg_send![text, length];
                let selection: ImeRange = msg_send![this, selectedRange];
                let Some(location) = selection.location.checked_sub(start as usize) else { return; };
                if location > length || selection.length > length - location { return; }
                let deletion = if selection.length != 0 {
                    ImeRange { location, length: selection.length }
                } else {
                    if location == 0 { return; }
                    // UITextView.deleteBackward cancels the whole marked text.
                    // Preserve composition and delete one native grapheme instead.
                    let range: ImeRange = msg_send![text, rangeOfComposedCharacterSequenceAtIndex: location - 1];
                    range
                };
                let empty: *mut AnyObject = msg_send![class!(NSString), string];
                let updated: *mut AnyObject = msg_send![text, stringByReplacingCharactersInRange: deletion, withString: empty];
                let updated: *mut AnyObject = msg_send![updated, retain];
                set_marked_text(this, sel!(setMarkedText:selectedRange:), updated,
                    ImeRange { location: deletion.location, length: 0 });
                let _: () = msg_send![updated, release];
                return;
            }
            native_ime_edit(this, || {
                let _: () = msg_send![super(this, class!(UITextView)), deleteBackward];
            });
            window.handle_delete_backward();
            if window.proxy_generation_is_current(generation) {
                sync_proxy_context(this, &window, false);
            }
        }

        // canBecomeFirstResponder must return Bool::YES
        unsafe extern "C" fn can_become_first_responder(_this: *mut AnyObject, _sel: Sel) -> Bool {
            Bool::YES
        }
        unsafe extern "C" fn caret_rect(this: *mut AnyObject, _sel: Sel, _position: *mut AnyObject) -> ObjcCGRect {
            msg_send![this, bounds]
        }
        unsafe {
            decl.add_method(sel!(caretRectForPosition:), caret_rect as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> ObjcCGRect);
            decl.add_method(sel!(firstRectForRange:), caret_rect as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> ObjcCGRect);
            decl.add_method(sel!(setMarkedText:selectedRange:), set_marked_text as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, ImeRange));
            decl.add_method(sel!(setAttributedMarkedText:selectedRange:), set_attributed_marked_text as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, ImeRange));
            decl.add_method(sel!(unmarkText), unmark_text as unsafe extern "C" fn(*mut AnyObject, Sel));
            decl.add_method(sel!(replaceRange:withText:), replace_range as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject));
            decl.add_method(
                sel!(hasText),
                has_text as unsafe extern "C" fn(*mut AnyObject, Sel) -> Bool,
            );
            decl.add_method(
                sel!(insertText:),
                insert_text as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            decl.add_method(
                sel!(deleteBackward),
                delete_backward as unsafe extern "C" fn(*mut AnyObject, Sel),
            );
            decl.add_method(
                sel!(canBecomeFirstResponder),
                can_become_first_responder as unsafe extern "C" fn(*mut AnyObject, Sel) -> Bool,
            );

        }

        decl.register();
    });

    class!(GPUITextInputView)
}

/// Handle touch events from the GPUIMetalView
fn handle_touches(view: *mut AnyObject, touches: *mut AnyObject, event: *mut AnyObject) {
    unsafe {
        // Get the window pointer from the view's ivar
        #[allow(deprecated)]
        let window_ptr: *mut std::ffi::c_void = *(*view).get_ivar(GPUI_WINDOW_IVAR);
        if window_ptr.is_null() {
            log::warn!("GPUI iOS: Touch event but no window pointer set");
            return;
        }

        let Some(window) = super::ffi::window_for_handle(window_ptr as usize) else {
            return;
        };

        // Get all touches from the set
        let all_touches: *mut AnyObject = msg_send![touches, allObjects];
        let count: usize = msg_send![all_touches, count];

        for i in 0..count {
            let touch: *mut AnyObject = msg_send![all_touches, objectAtIndex: i];
            window.handle_touch(touch, event);
        }
    }
}

/// iOS Window backed by UIWindow + UIViewController.
/// Distance (logical px) the finger must travel before a touch
/// is promoted from a potential tap to a scroll gesture.
const SCROLL_SLOP: f32 = 8.0;

/// Tracks the current touch gesture state machine.
///
/// This distinguishes taps (short, stationary touches) from scroll gestures
/// (finger drags). The same pattern is used on Android.
#[derive(Clone, Copy, Debug)]
enum TouchState {
    /// No active touch.
    Idle,
    /// Finger is down but hasn't moved beyond the slop threshold.
    Pending { start_x: f32, start_y: f32 },
    /// Finger has moved beyond the threshold — we are scrolling.
    Scrolling { prev_x: f32, prev_y: f32 },
    /// A GPUI control claimed the drag before scrolling started.
    Dragging { start: Point<Pixels> },
}

#[allow(clippy::type_complexity)]
pub(crate) struct IosWindow {
    inner: Rc<IosWindowInner>,
}

impl std::ops::Deref for IosWindow {
    type Target = IosWindowInner;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl IosWindow {
    pub fn new(handle: AnyWindowHandle, params: WindowParams) -> anyhow::Result<Self> {
        Ok(Self {
            inner: Rc::new(IosWindowInner::new(handle, params)?),
        })
    }
    pub(crate) fn register_with_ffi(&self) {
        self.inner.register_with_ffi();
    }
}

#[allow(clippy::type_complexity)]
pub(crate) struct IosWindowInner {
    gpui_handle: AnyWindowHandle,
    handle_id: Cell<usize>,
    pub(crate) closed: Cell<bool>,
    /// The UIWindow object
    window: *mut AnyObject,
    /// The UIViewController
    view_controller: *mut AnyObject,
    /// The Metal-backed UIView
    view: *mut AnyObject,
    /// The hidden text input view for keyboard input
    text_input_view: *mut AnyObject,
    /// Current bounds in pixels
    bounds: Cell<Bounds<Pixels>>,
    /// Scale factor
    scale_factor: Cell<f32>,
    /// Input handler for text input
    input_handler: Callback<PlatformInputHandler>,
    proxy_input_session: RefCell<ProxyInputSession<gpui::FocusId>>,
    proxy_context: RefCell<
        Option<(
            Option<mobile_events::TextInputOwner>,
            std::ops::Range<usize>,
        )>,
    >,
    proxy_base: Cell<Option<usize>>,
    proxy_mapping_valid: Cell<bool>,
    proxy_text: RefCell<String>,
    input_configuration: RefCell<gpui::TextInputConfiguration>,
    /// Callback for frame requests
    /// Note: pub(super) to allow ffi.rs to access this for the display link callback
    pub(super) request_frame_callback: Callback<Box<dyn FnMut(RequestFrameOptions)>>,
    pub(super) frame_demand: Rc<crate::frame_demand::FrameDemand>,
    /// Callback for input events
    input_callback: Callback<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    /// Callback for active status changes
    active_status_callback: Callback<Box<dyn FnMut(bool)>>,
    visibility_callback: Callback<Box<dyn FnMut(gpui::WindowVisibility)>>,
    app_active: Cell<bool>,
    surface_available: Cell<bool>,
    keyboard_observers: RefCell<Vec<*mut AnyObject>>,
    /// Callback for hover status changes (not really applicable on iOS)
    hover_status_callback: Callback<Box<dyn FnMut(bool)>>,
    /// Callback for resize events
    resize_callback: Callback<Box<dyn FnMut(Size<Pixels>, f32)>>,
    /// Callback for move events (not applicable on iOS)
    moved_callback: Callback<Box<dyn FnMut()>>,
    /// Callback for should close
    should_close_callback: Callback<Box<dyn FnMut() -> bool>>,
    /// Callback for hit test
    hit_test_callback: Callback<Box<dyn FnMut() -> Option<WindowControlArea>>>,
    /// Callback for close
    close_callback: Callback<Box<dyn FnOnce()>>,
    /// Callback for appearance changes
    appearance_changed_callback: Callback<Box<dyn FnMut()>>,
    /// Current mouse position (from touch)
    mouse_position: Cell<Point<Pixels>>,
    /// Current modifiers
    modifiers: Cell<Modifiers>,
    capslock: Cell<bool>,
    /// Track if a touch is currently pressed
    touch_pressed: Cell<bool>,
    /// Touch gesture state machine — distinguishes taps from scroll drags.
    touch_state: Cell<TouchState>,
    mobile_events: RefCell<MobileEventState>,
    legacy_multitouch: Cell<bool>,
    last_pinch_scale: Cell<Option<f32>>,
    keyboard_bounds: Cell<Option<Bounds<Pixels>>>,
    tablet_keyboard_guide: Cell<*mut AnyObject>,
    /// Latest complete observation, including floating keyboard geometry.
    mobile_insets: RefCell<WindowInsets>,
    insets_changed_callback: Callback<Box<dyn FnMut(gpui::WindowInsets)>>,
    /// Velocity tracker — records recent touch samples during drag gestures
    /// so we can compute the release velocity when the finger lifts.
    velocity_tracker: RefCell<VelocityTracker>,
    /// Momentum scroller — produces decelerating scroll deltas after a fling
    /// gesture, driven by the CADisplayLink frame callback.
    momentum_scroller: RefCell<MomentumScroller>,
    /// The wgpu renderer (Metal backend on iOS).
    /// Wrapped in a `Mutex<Option<…>>` so that `draw()` (called from the
    /// `request_frame` callback) can acquire a mutable reference without
    /// conflicting with the outer `&self` borrow.
    renderer: Mutex<Option<WgpuRenderer>>,
    gpu_context: GpuContext,
}

// Required for raw_window_handle
unsafe impl Send for IosWindow {}
unsafe impl Sync for IosWindow {}

impl IosWindowInner {
    fn proxy_generation(&self) -> u64 {
        self.proxy_input_session.borrow().generation()
    }

    fn proxy_generation_is_current(&self, generation: u64) -> bool {
        self.proxy_input_session.borrow().is_current(generation)
    }

    fn apply_input_configuration(&self) {
        let configuration = self.input_configuration.borrow().clone();
        let (correct, capitalize, spellcheck, action) =
            mobile_events::ios_input_traits(&configuration);
        unsafe {
            let _: () = msg_send![self.text_input_view, setAutocorrectionType: correct];
            let _: () = msg_send![self.text_input_view, setAutocapitalizationType: capitalize];
            let _: () = msg_send![self.text_input_view, setSpellCheckingType: spellcheck];
            let _: () = msg_send![self.text_input_view, setReturnKeyType: action];
            let _: () = msg_send![self.text_input_view, reloadInputViews];
        }
    }
    pub(crate) fn native_handle(&self) -> usize {
        self.handle_id.get()
    }

    pub(crate) fn active_handle(&self) -> Option<AnyWindowHandle> {
        if self.closed.get() || !self.app_active.get() {
            return None;
        }
        let key: Bool = unsafe { msg_send![self.window, isKeyWindow] };
        key.as_bool().then_some(self.gpui_handle)
    }

    fn dispatch_input(&self, event: PlatformInput) -> bool {
        self.input_callback
            .call(|| !self.closed.get(), |callback| callback(event))
            .is_some_and(|result| result.default_prevented)
    }

    fn dispatch_touch_drag(
        &self,
        phase: gpui::TouchPhase,
        start_position: Point<Pixels>,
        position: Point<Pixels>,
    ) -> bool {
        self.dispatch_input(PlatformInput::TouchDrag(gpui::TouchDragEvent {
            phase,
            start_position,
            position,
        }))
    }

    pub(crate) fn can_render(&self) -> bool {
        !self.closed.get() && self.app_active.get() && self.surface_available.get()
    }

    fn notify_visibility(&self, previously_visible: bool) {
        if previously_visible != self.can_render() {
            self.visibility_callback.call(
                || !self.closed.get(),
                |callback| {
                    callback(if self.can_render() {
                        gpui::WindowVisibility::Visible
                    } else {
                        gpui::WindowVisibility::Hidden
                    });
                },
            );
        }
    }

    /// UIKit retains the view across backgrounding; surface loss is tied to actual
    /// window attachment (or zero drawable size), independently of app activation.
    fn sync_surface_availability(&self) {
        if self.closed.get() {
            return;
        }
        unsafe {
            let native_window: *mut AnyObject = msg_send![self.view, window];
            let bounds: ObjcCGRect = msg_send![self.view, bounds];
            let available = !native_window.is_null() && bounds.width > 0.0 && bounds.height > 0.0;
            if available == self.surface_available.get() {
                return;
            }
            let previously_visible = self.can_render();
            if available {
                let instance = self
                    .gpu_context
                    .borrow()
                    .as_ref()
                    .map(|context| context.instance.clone());
                let Some(instance) = instance else {
                    return;
                };
                let scale: f64 = msg_send![self.view, contentScaleFactor];
                let config = WgpuSurfaceConfig {
                    size: size(
                        DevicePixels((bounds.width * scale) as i32),
                        DevicePixels((bounds.height * scale) as i32),
                    ),
                    transparent: false,
                    preferred_present_mode: None,
                };
                let mut renderer = self.renderer.lock();
                let Some(renderer) = renderer.as_mut() else {
                    return;
                };
                if let Err(error) = renderer.replace_surface(
                    &RawIosWindow {
                        view: self.view.cast(),
                    },
                    config,
                    &instance,
                ) {
                    log::error!("Failed to recreate UIKit Metal surface: {error:#}");
                    return;
                }
            } else {
                if let Some(renderer) = self.renderer.lock().as_mut() {
                    renderer.unconfigure_surface();
                }
                self.cancel_legacy_touch();
                if self.closed.get() {
                    return;
                }
                self.legacy_multitouch.set(false);
                self.last_pinch_scale.set(None);
            }
            self.surface_available.set(available);
            if available {
                self.frame_demand.clear();
                self.frame_demand.wake();
            }
            let phase = if available {
                SurfaceLifecycle::Available
            } else {
                SurfaceLifecycle::Lost
            };
            let events = self
                .mobile_events
                .borrow_mut()
                .surface(phase, std::time::Duration::ZERO);
            self.emit_mobile_events(events);
            self.notify_visibility(previously_visible);
        }
    }

    fn emit_mobile_events(&self, events: Vec<MobileEvent>) {
        if !self.closed.get() {
            mobile_events::dispatch_batch(self.handle_id.get() as u64, events);
        }
    }

    fn cancel_legacy_touch(&self) {
        let state = self.touch_state.replace(TouchState::Idle);
        self.touch_pressed.set(false);
        self.velocity_tracker.borrow_mut().reset();
        self.momentum_scroller.borrow_mut().cancel();
        if let TouchState::Dragging { start } = state {
            self.dispatch_touch_drag(
                gpui::TouchPhase::Cancelled,
                start,
                self.mouse_position.get(),
            );
        } else if matches!(state, TouchState::Scrolling { .. }) {
            self.dispatch_input(PlatformInput::Scroll(gpui::ScrollEvent {
                position: self.mouse_position.get(),
                delta: gpui::ScrollDelta::Pixels(Point::default()),
                modifiers: self.modifiers.get(),
                touch_phase: gpui::TouchPhase::Cancelled,
                source: gpui::ScrollSource::Touch,
            }));
        }
        if !matches!(state, TouchState::Idle) {
            self.clear_touch_hover();
        }
    }

    fn clear_touch_hover(&self) {
        // The lifted finger must stop participating in hit tests.
        let position = point(px(-1.), px(-1.));
        let modifiers = self.modifiers.get();
        self.mouse_position.set(position);
        self.dispatch_input(PlatformInput::MouseMove(gpui::MouseMoveEvent {
            position,
            modifiers,
            pressed_button: None,
        }));
        self.dispatch_input(PlatformInput::MouseExited(gpui::MouseExitEvent {
            position,
            modifiers,
            pressed_button: None,
        }));
    }

    fn emit_insets(&self) {
        if self.closed.get() {
            return;
        }
        let (top, bottom, left, right) = self.safe_area_insets();
        let insets = WindowInsets {
            safe_area: gpui::Edges {
                top: px(top),
                bottom: px(bottom),
                left: px(left),
                right: px(right),
            },
            keyboard: self.keyboard_bounds.get(),
            keyboard_visible: Some(self.keyboard_bounds.get().is_some()),
            keyboard_insets: None,
        };
        let previous = self.mobile_insets.replace(insets.clone());
        let changed = previous != insets;
        let platform_insets = platform_insets(self.bounds.get(), &insets);
        self.emit_mobile_events(vec![MobileEvent::Insets(insets)]);
        if changed {
            self.insets_changed_callback
                .call(|| !self.closed.get(), |callback| callback(platform_insets));
            crate::render_request::request_force_render();
        }
    }

    pub fn new(handle: AnyWindowHandle, _params: WindowParams) -> anyhow::Result<Self> {
        // Create the window on the main screen
        let screen = IosDisplay::main();
        let screen_bounds = screen.bounds();
        let scale_factor = screen.scale();

        unsafe {
            // Create UIWindow
            let screen_obj: *mut AnyObject = msg_send![class!(UIScreen), mainScreen];
            let screen_bounds_cg: ObjcCGRect = msg_send![screen_obj, bounds];
            let window: *mut AnyObject = msg_send![class!(UIWindow), alloc];
            let window: *mut AnyObject = msg_send![window, initWithFrame: screen_bounds_cg];

            // Create our custom UIViewController subclass that supports
            // dynamic `preferredStatusBarStyle` overrides.
            let vc_class = register_view_controller_class();
            let view_controller: *mut AnyObject = msg_send![vc_class, alloc];
            let view_controller: *mut AnyObject = msg_send![view_controller, init];

            // Create our custom Metal view using the registered class
            let metal_view_class = register_metal_view_class();
            let view: *mut AnyObject = msg_send![metal_view_class, alloc];
            let view: *mut AnyObject = msg_send![view, initWithFrame: screen_bounds_cg];

            // Configure the Metal layer — wgpu will use it for rendering but
            // we still need to set contentsScale so the drawable size is correct.
            let layer: *mut AnyObject = msg_send![view, layer];
            let scale: core_graphics::base::CGFloat = msg_send![screen_obj, scale];
            let _: () = msg_send![layer, setContentsScale: scale];
            // Keep the drawable at its true scale while UIKit animates the view bounds.
            let visual_top_left = crate::ios::util::nsstring("bottomLeft");
            let _: () = msg_send![layer, setContentsGravity: visual_top_left];

            // Auto-resize the Metal view when the parent view changes size
            // (e.g. rotation). UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight
            let _: () = msg_send![view, setAutoresizingMask: 18_usize]; // 0x02 | 0x10

            // Enable user interaction on the Metal view for touch handling
            let _: () = msg_send![view, setUserInteractionEnabled: true];
            let _: () = msg_send![view, setMultipleTouchEnabled: true];
            let recognizer: *mut AnyObject = msg_send![class!(UIPinchGestureRecognizer), alloc];
            let recognizer: *mut AnyObject =
                msg_send![recognizer, initWithTarget: view, action: sel!(gpuiPinch:)];
            let _: () = msg_send![recognizer, setCancelsTouchesInView: false];
            let _: () = msg_send![recognizer, setDelaysTouchesEnded: false];
            let _: () = msg_send![view, addGestureRecognizer: recognizer];
            let _: () = msg_send![recognizer, release];

            // Set the view as the view controller's view
            let _: () = msg_send![view_controller, setView: view];

            // Set the root view controller
            let _: () = msg_send![window, setRootViewController: view_controller];

            // Make the window visible
            let _: () = msg_send![window, makeKeyAndVisible];

            // Create a hidden text input view for keyboard handling.
            // Uses our custom GPUITextInputView which implements UIKeyInput
            // so iOS actually routes keyboard text to us.
            let text_input_class = register_text_input_view_class();
            let text_input_view: *mut AnyObject = msg_send![text_input_class, alloc];
            let text_input_frame = ObjcCGRect::new(0.0, 0.0, 1.0, 1.0);
            let text_input_view: *mut AnyObject =
                msg_send![text_input_view, initWithFrame: text_input_frame];
            input_traits::initialize(text_input_view);
            #[cfg(debug_assertions)]
            input_traits::verify_live_proxy(text_input_view);
            let _: () = msg_send![text_input_view, setAlpha: 0.01_f64];
            let clear: *mut AnyObject = msg_send![class!(UIColor), clearColor];
            let _: () = msg_send![text_input_view, setTextColor: clear];
            let _: () = msg_send![text_input_view, setBackgroundColor: clear];
            let _: () = msg_send![text_input_view, setScrollEnabled: Bool::NO];
            let _: () = msg_send![text_input_view, setUserInteractionEnabled: true];
            let _: () = msg_send![view, addSubview: text_input_view];

            // --- Initialise the wgpu renderer (Metal backend) ---------------
            let pixel_w = (screen_bounds_cg.width * scale) as i32;
            let pixel_h = (screen_bounds_cg.height * scale) as i32;

            let ios_window = Self {
                gpui_handle: handle,
                handle_id: Cell::new(0),
                closed: Cell::new(false),
                window,
                view_controller,
                view,
                text_input_view,
                bounds: Cell::new(screen_bounds),
                scale_factor: Cell::new(scale_factor),
                input_handler: Callback::default(),
                proxy_input_session: RefCell::new(ProxyInputSession::new()),
                proxy_context: RefCell::new(None),
                proxy_base: Cell::new(None),
                proxy_mapping_valid: Cell::new(false),
                proxy_text: RefCell::new(String::new()),
                input_configuration: RefCell::new(gpui::TextInputConfiguration::default()),
                request_frame_callback: Callback::default(),
                frame_demand: Rc::new(crate::frame_demand::FrameDemand::default()),
                input_callback: Callback::default(),
                active_status_callback: Callback::default(),
                visibility_callback: Callback::default(),
                app_active: Cell::new(true),
                surface_available: Cell::new(false),
                keyboard_observers: RefCell::new(Vec::new()),
                hover_status_callback: Callback::default(),
                resize_callback: Callback::default(),
                moved_callback: Callback::default(),
                should_close_callback: Callback::default(),
                hit_test_callback: Callback::default(),
                close_callback: Callback::default(),
                appearance_changed_callback: Callback::default(),
                mouse_position: Cell::new(Point::default()),
                modifiers: Cell::new(Modifiers::default()),
                capslock: Cell::new(false),
                touch_pressed: Cell::new(false),
                touch_state: Cell::new(TouchState::Idle),
                mobile_events: RefCell::new(MobileEventState::default()),
                legacy_multitouch: Cell::new(false),
                last_pinch_scale: Cell::new(None),
                keyboard_bounds: Cell::new(None),
                tablet_keyboard_guide: Cell::new(std::ptr::null_mut()),
                mobile_insets: RefCell::new(WindowInsets::default()),
                insets_changed_callback: Callback::default(),
                velocity_tracker: RefCell::new(VelocityTracker::new()),
                momentum_scroller: RefCell::new(MomentumScroller::new()),
                renderer: Mutex::new(None),
                gpu_context: Rc::new(RefCell::new(None)),
            };

            // Create the wgpu renderer using the Metal backend.
            //
            // `gpui_wgpu::WgpuContext::instance()` only enables Vulkan+GL,
            // so we create our own wgpu instance with Metal enabled, build
            // a surface from the UIView's raw window handle, construct the
            // WgpuContext with that instance, and finally create the renderer.
            let config = WgpuSurfaceConfig {
                size: size(DevicePixels(pixel_w), DevicePixels(pixel_h)),
                transparent: false,
                preferred_present_mode: None,
            };

            let metal_instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::METAL,
                flags: wgpu::InstanceFlags::default(),
                backend_options: wgpu::BackendOptions::default(),
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
                display: Some(Box::new(RawIosWindow {
                    view: ios_window.view as *mut c_void,
                })),
            });

            let raw_window = RawIosWindow {
                view: ios_window.view as *mut c_void,
            };

            // Build a temporary surface for WgpuContext initialisation
            // (adapter selection needs a surface to test compatibility).
            let window_handle = raw_window
                .window_handle()
                .expect("iOS window handle unavailable");

            let target = wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: None,
                raw_window_handle: window_handle.as_raw(),
            };

            let surface_result = metal_instance.create_surface_unsafe(target);
            match surface_result {
                Ok(surface) => match WgpuContext::new(metal_instance, &surface, &config, None) {
                    Ok(context) => {
                        // Pre-populate gpu_context so WgpuRenderer::new()
                        // reuses our Metal-backed context (and its instance)
                        // instead of creating a Vulkan+GL one.
                        *ios_window.gpu_context.borrow_mut() = Some(context);
                        let gpu_context = Rc::clone(&ios_window.gpu_context);
                        drop(surface); // no longer needed — new() creates its own

                        match WgpuRenderer::new(gpu_context, &raw_window, config, None) {
                            Ok(renderer) => {
                                log::info!("iOS wgpu renderer created (Metal)");
                                *ios_window.renderer.lock() = Some(renderer);
                            }
                            Err(e) => {
                                log::error!("Failed to create iOS wgpu renderer: {e:#}");
                                return Err(e);
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to create iOS WgpuContext: {e:#}");
                        return Err(e);
                    }
                },
                Err(e) => {
                    log::error!("Failed to create iOS wgpu Metal surface: {e:#}");
                    return Err(e.into());
                }
            }

            Ok(ios_window)
        }
    }

    /// Get the raw pointer to the UIViewController.
    pub fn view_controller_ptr(&self) -> *mut AnyObject {
        self.view_controller
    }

    /// Get the raw pointer to the GPUIMetalView.
    pub fn metal_view_ptr(&self) -> *mut AnyObject {
        self.view
    }

    /// Register this window with the FFI layer after it's been stored.
    /// This must be called after the window is placed at a stable address
    /// (e.g., in a Box or Arc).
    pub(crate) fn register_with_ffi(self: &Rc<Self>) {
        if self.handle_id.get() != 0 {
            return;
        }
        self.handle_id.set(super::ffi::register_window(self));

        // Set the window pointer on the view so touch events can find us,
        // and on the text input view so keyboard input can find us.
        unsafe {
            let window_ptr = self.handle_id.get() as *mut std::ffi::c_void;
            #[allow(deprecated)]
            {
                *(*self.view).get_mut_ivar::<*mut c_void>(GPUI_WINDOW_IVAR) = window_ptr;
            }
            #[allow(deprecated)]
            {
                *(*self.text_input_view).get_mut_ivar::<*mut c_void>(GPUI_WINDOW_IVAR) = window_ptr;
            }
            log::info!(
                "GPUI iOS: Set window pointer {:p} on view {:p} and text input {:p}",
                window_ptr,
                self.view,
                self.text_input_view
            );
        }

        // Listen for keyboard show/hide so we can expose the keyboard height.
        self.register_keyboard_observers();
        self.sync_surface_availability();
    }

    fn update_tablet_keyboard_bounds(&self) {
        let guide = self.tablet_keyboard_guide.get();
        if guide.is_null() {
            return;
        }
        unsafe {
            let frame: ObjcCGRect = msg_send![guide, layoutFrame];
            let view: ObjcCGRect = msg_send![self.view, bounds];
            let (_, safe_bottom, _, _) = self.safe_area_insets();
            // With the keyboard hidden, UIKit leaves a guide as tall as the bottom safe area.
            let x = frame.x.max(view.x);
            let y = frame.y.max(view.y);
            let width = (frame.x + frame.width).min(view.x + view.width) - x;
            let height = (frame.y + frame.height).min(view.y + view.height) - y;
            let keyboard = (width > 0.0 && height > f64::from(safe_bottom) + 0.5).then(|| {
                Bounds::new(
                    point(px(x as f32), px(y as f32)),
                    size(px(width as f32), px(height as f32)),
                )
            });
            self.keyboard_bounds.set(keyboard);
            crate::set_keyboard_height(
                keyboard.map_or(0.0, |bounds| f32::from(bounds.size.height)),
            );
        }
    }

    fn install_tablet_keyboard_guide(&self) {
        if self.closed.get() {
            return;
        }
        unsafe {
            let guide: *mut AnyObject = msg_send![self.view, keyboardLayoutGuide];
            if guide.is_null() {
                return;
            }
            self.tablet_keyboard_guide.set(guide);
            let _: () = msg_send![guide, setFollowsUndockedKeyboard: true];
            // A constrained, inert view makes guide movements participate in UIKit layout.
            let marker: *mut AnyObject = msg_send![class!(UIView), new];
            let _: () = msg_send![marker, setHidden: true];
            let _: () = msg_send![marker, setTranslatesAutoresizingMaskIntoConstraints: false];
            let _: () = msg_send![self.view, addSubview: marker];
            for anchor in [
                sel!(leadingAnchor),
                sel!(trailingAnchor),
                sel!(topAnchor),
                sel!(bottomAnchor),
            ] {
                let a: *mut AnyObject = objc2::msg_send![marker, performSelector: anchor];
                let b: *mut AnyObject = objc2::msg_send![guide, performSelector: anchor];
                let constraint: *mut AnyObject = msg_send![a, constraintEqualToAnchor: b];
                let _: () = msg_send![constraint, setActive: true];
            }
            let _: () = msg_send![marker, release];
        }
    }

    /// Register for keyboard show/hide notifications so we can track the
    /// keyboard height and allow the UI to shift content above the keyboard.
    pub(crate) fn register_keyboard_observers(&self) {
        unsafe {
            let device: *mut AnyObject = msg_send![class!(UIDevice), currentDevice];
            let idiom: isize = msg_send![device, userInterfaceIdiom];
            let supported: bool =
                msg_send![self.view, respondsToSelector: sel!(keyboardLayoutGuide)];
            if idiom == 1 && supported {
                // UIKit can send input-mode notifications while installing the guide.
                // Install it after GPUI releases its startup App borrow.
                unsafe extern "C" fn install(handle: *mut c_void) {
                    if let Some(window) = super::ffi::window_for_handle(handle as usize) {
                        window.install_tablet_keyboard_guide();
                    }
                }
                super::dispatcher::dispatch_async_f(
                    super::dispatcher::dispatch_get_main_queue(),
                    self.handle_id.get() as usize as *mut c_void,
                    Some(install),
                );
                return;
            }

            let notification_center: *mut AnyObject =
                msg_send![class!(NSNotificationCenter), defaultCenter];

            let show_name = crate::ios::util::nsstring("UIKeyboardWillChangeFrameNotification");
            let hide_name = crate::ios::util::nsstring("UIKeyboardWillHideNotification");
            let window_address = self.handle_id.get();

            // Block that fires when the keyboard appears — extracts the
            // end-frame height and stores it in the global atomic.
            let show_block = block2::RcBlock::new(move |notification: *mut AnyObject| {
                if notification.is_null() {
                    return;
                }
                let user_info: *mut AnyObject = msg_send![notification, userInfo];
                if user_info.is_null() {
                    return;
                }
                let frame_key = crate::ios::util::nsstring("UIKeyboardFrameEndUserInfoKey");
                let frame_value: *mut AnyObject = msg_send![user_info, objectForKey: frame_key];
                // frame_key is autoreleased by util::nsstring — no manual release needed
                let _ = frame_key;
                if frame_value.is_null() {
                    return;
                }
                let frame: ObjcCGRect = msg_send![frame_value, CGRectValue];
                let Some(window) = super::ffi::window_for_handle(window_address) else {
                    return;
                };
                let screen: *mut AnyObject = msg_send![window.window, screen];
                let coordinate_space: *mut AnyObject = msg_send![screen, coordinateSpace];
                let local: ObjcCGRect = msg_send![window.view, convertRect: frame, fromCoordinateSpace: coordinate_space];
                let bounds: ObjcCGRect = msg_send![window.view, bounds];
                let x = local.x.max(bounds.x);
                let y = local.y.max(bounds.y);
                let width = (local.x + local.width).min(bounds.x + bounds.width) - x;
                let height = (local.y + local.height).min(bounds.y + bounds.height) - y;
                window
                    .keyboard_bounds
                    .set((width > 0.0 && height > 0.0).then(|| Bounds {
                        origin: point(px(x as f32), px(y as f32)),
                        size: size(px(width as f32), px(height as f32)),
                    }));
                crate::set_keyboard_height(height.max(0.0) as f32);
                window.emit_insets();
            });

            let hide_block = block2::RcBlock::new(move |_notification: *mut AnyObject| {
                log::info!("GPUI iOS: Keyboard will hide");
                crate::set_keyboard_height(0.0);
                let Some(window) = super::ffi::window_for_handle(window_address) else {
                    return;
                };
                window.keyboard_bounds.set(None);
                window.emit_insets();
            });

            let show_observer: *mut AnyObject = msg_send![notification_center,
                addObserverForName: show_name,
                object: std::ptr::null::<AnyObject>(),
                queue: std::ptr::null::<AnyObject>(),
                usingBlock: &*show_block
            ];
            let hide_observer: *mut AnyObject = msg_send![notification_center,
                addObserverForName: hide_name,
                object: std::ptr::null::<AnyObject>(),
                queue: std::ptr::null::<AnyObject>(),
                usingBlock: &*hide_block
            ];
            // show_name and hide_name are autoreleased by util::nsstring

            self.keyboard_observers
                .borrow_mut()
                .extend([show_observer, hide_observer]);
        }
    }

    /// Handle a touch event from UIKit.
    ///
    /// Uses a state machine to distinguish **taps** from **drag gestures**:
    ///
    ///   DOWN  → record start position, enter "pending" (NO MouseDown yet)
    ///   MOVE  → if finger moved > threshold → switch to "scrolling",
    ///           emit `ScrollWheel` deltas (for scrollable containers) AND
    ///           `MouseMove` (for interactive canvas screens like Animations)
    ///   UP    → if still "pending" → emit `MouseDown` + `MouseUp` (tap)
    ///           if "scrolling"   → emit final `ScrollWheel` (Ended) +
    ///           `MouseUp` (so drag-to-throw works)
    ///
    /// MouseDown is **deferred** until finger-up so that starting a scroll
    /// near a button or tab doesn't accidentally trigger navigation.
    /// Interactive screens use `MouseMove` to track the finger during drags
    /// and `MouseUp` to detect the end of a throw/drag gesture.
    pub fn handle_touch(&self, touch: *mut AnyObject, _event: *mut AnyObject) {
        if self.closed.get() {
            return;
        }
        let mut position = touch_location_in_view(touch, self.view);
        let phase = touch_phase(touch);
        let tap_count = touch_tap_count(touch);
        let modifiers = self.modifiers.get();

        let native_time: f64 = unsafe { msg_send![touch, timestamp] };
        let timestamp = std::time::Duration::try_from_secs_f64(native_time).unwrap_or_default();
        let raw_phase = match phase {
            UITouchPhase::Began => gpui::TouchPhase::Started,
            UITouchPhase::Moved => gpui::TouchPhase::Moved,
            UITouchPhase::Ended => gpui::TouchPhase::Ended,
            UITouchPhase::Cancelled => gpui::TouchPhase::Cancelled,
            UITouchPhase::Stationary => return,
        };
        let raw =
            self.mobile_events
                .borrow_mut()
                .touch(touch as u64, raw_phase, position, timestamp);
        if let Some(raw) = raw {
            if let MobileEvent::Touch { event, .. } = &raw {
                position = event.position;
            }
            self.emit_mobile_events(vec![raw]);
        } else {
            return;
        }
        let pointer_count = self.mobile_events.borrow().active_pointer_count();
        if crate::mobile_events::raw_touch_only() {
            self.cancel_legacy_touch();
            return;
        }
        if self.closed.get() {
            return;
        }
        if pointer_count > 1 {
            self.legacy_multitouch.set(true);
        }
        if matches!(phase, UITouchPhase::Cancelled) || self.legacy_multitouch.get() {
            self.cancel_legacy_touch();
            if pointer_count == 0 {
                self.legacy_multitouch.set(false);
            }
            return;
        }

        let logical_x: f32 = position.x.into();
        let logical_y: f32 = position.y.into();

        self.mouse_position.set(position);

        let mut ts = self.touch_state.get();

        let emit = |input: PlatformInput| self.dispatch_input(input);

        match phase {
            UITouchPhase::Began => {
                self.touch_pressed.set(true);
                // Cancel any active momentum fling — the user touched the
                // screen again, so inertia scrolling must stop immediately.
                self.momentum_scroller.borrow_mut().cancel();
                self.velocity_tracker.borrow_mut().reset();

                ts = TouchState::Pending {
                    start_x: logical_x,
                    start_y: logical_y,
                };
                // Do NOT emit MouseDown here — wait until we know whether
                // this is a tap or a scroll.  Emitting MouseDown immediately
                // causes accidental navigation when the user starts scrolling
                // near a button/tab.
                //
                // - Tap (finger lifts within slop) → emit MouseDown + MouseUp
                //   together in Ended phase.
                // - Scroll (finger exceeds slop) → emit only MouseMove +
                //   ScrollWheel, no MouseDown.
            }

            UITouchPhase::Moved => {
                // Record every move for velocity estimation.
                self.velocity_tracker
                    .borrow_mut()
                    .record(logical_x, logical_y);

                match ts {
                    TouchState::Pending { start_x, start_y } => {
                        let dx = logical_x - start_x;
                        let dy = logical_y - start_y;
                        let distance = (dx * dx + dy * dy).sqrt();

                        if distance > SCROLL_SLOP {
                            let start = gpui::point(gpui::px(start_x), gpui::px(start_y));
                            self.touch_state.set(TouchState::Dragging { start });
                            let claimed = self.dispatch_touch_drag(
                                gpui::TouchPhase::Started,
                                start,
                                position,
                            );
                            if claimed || !self.touch_pressed.get() {
                                return;
                            }
                            // Promote to scrolling — emit the first scroll
                            // delta from the start position.
                            ts = TouchState::Scrolling {
                                prev_x: logical_x,
                                prev_y: logical_y,
                            };
                            emit(PlatformInput::Scroll(gpui::ScrollEvent {
                                position,
                                delta: gpui::ScrollDelta::Pixels(gpui::point(
                                    gpui::px(dx),
                                    gpui::px(dy),
                                )),
                                modifiers,
                                touch_phase: gpui::TouchPhase::Started,
                                source: gpui::ScrollSource::Touch,
                            }));
                        }
                        // Always emit MouseMove so interactive screens can
                        // track finger position (e.g. drag line in Animations,
                        // gradient control in Shaders).
                        emit(PlatformInput::MouseMove(gpui::MouseMoveEvent {
                            position,
                            modifiers,
                            pressed_button: Some(gpui::MouseButton::Left),
                        }));
                    }
                    TouchState::Scrolling { prev_x, prev_y } => {
                        let dx = logical_x - prev_x;
                        let dy = logical_y - prev_y;
                        ts = TouchState::Scrolling {
                            prev_x: logical_x,
                            prev_y: logical_y,
                        };
                        // Scroll event for scrollable containers.
                        emit(PlatformInput::Scroll(gpui::ScrollEvent {
                            position,
                            delta: gpui::ScrollDelta::Pixels(gpui::point(
                                gpui::px(dx),
                                gpui::px(dy),
                            )),
                            modifiers,
                            touch_phase: gpui::TouchPhase::Moved,
                            source: gpui::ScrollSource::Touch,
                        }));
                        // MouseMove for interactive screens.
                        emit(PlatformInput::MouseMove(gpui::MouseMoveEvent {
                            position,
                            modifiers,
                            pressed_button: Some(gpui::MouseButton::Left),
                        }));
                    }
                    TouchState::Dragging { start } => {
                        self.dispatch_touch_drag(gpui::TouchPhase::Moved, start, position);
                        return;
                    }
                    TouchState::Idle => {
                        // Spurious move without a preceding down — ignore.
                    }
                }
            }

            UITouchPhase::Ended => {
                self.touch_pressed.set(false);
                match ts {
                    TouchState::Pending { start_x, start_y } => {
                        // Finger lifted without exceeding slop → tap.
                        // Emit MouseDown + MouseUp together at the original
                        // down position so hit-testing matches the initial
                        // touch point.
                        self.velocity_tracker.borrow_mut().reset();
                        let tap_pos = gpui::point(gpui::px(start_x), gpui::px(start_y));
                        emit(PlatformInput::MouseDown(gpui::MouseDownEvent {
                            button: gpui::MouseButton::Left,
                            position: tap_pos,
                            modifiers,
                            click_count: tap_count as usize,
                            first_mouse: false,
                        }));
                        emit(PlatformInput::MouseUp(gpui::MouseUpEvent {
                            button: gpui::MouseButton::Left,
                            position: tap_pos,
                            modifiers,
                            click_count: tap_count as usize,
                        }));
                    }
                    TouchState::Scrolling { prev_x, prev_y } => {
                        // End the active touch-scroll gesture.
                        let dx = logical_x - prev_x;
                        let dy = logical_y - prev_y;
                        emit(PlatformInput::Scroll(gpui::ScrollEvent {
                            position,
                            delta: gpui::ScrollDelta::Pixels(gpui::point(
                                gpui::px(dx),
                                gpui::px(dy),
                            )),
                            modifiers,
                            touch_phase: gpui::TouchPhase::Ended,
                            source: gpui::ScrollSource::Touch,
                        }));
                        // Also emit MouseUp so interactive screens can
                        // detect the end of a drag (e.g. fling a ball).
                        emit(PlatformInput::MouseUp(gpui::MouseUpEvent {
                            button: gpui::MouseButton::Left,
                            position,
                            modifiers,
                            click_count: 1,
                        }));

                        // ── Start momentum / inertia scrolling ───────────
                        // Compute release velocity from recent touch samples
                        // and kick off the momentum scroller.  Subsequent
                        // frames will pump synthetic ScrollWheel events via
                        // `pump_momentum()` until velocity decays below the
                        // threshold.
                        let (vx, vy) = self.velocity_tracker.borrow().velocity();
                        self.velocity_tracker.borrow_mut().reset();
                        self.momentum_scroller
                            .borrow_mut()
                            .fling(vx, vy, logical_x, logical_y);
                        if self.momentum_scroller.borrow().is_active() {
                            self.frame_demand.wake();
                        }
                    }
                    TouchState::Dragging { start } => {
                        self.touch_state.set(TouchState::Idle);
                        self.dispatch_touch_drag(gpui::TouchPhase::Ended, start, position);
                        self.clear_touch_hover();
                        return;
                    }
                    TouchState::Idle => {}
                }
                ts = TouchState::Idle;
            }

            UITouchPhase::Stationary | UITouchPhase::Cancelled => {
                // No change — ignore.
                return;
            }
        }

        self.touch_state.set(ts);
        if matches!(phase, UITouchPhase::Ended) {
            self.clear_touch_hover();
        }
    }

    /// Query the safe area insets from the UIView.
    ///
    /// Returns `(top, bottom, left, right)` in logical points.
    /// These represent the areas occupied by system UI (status bar,
    /// home indicator, camera notch) that content should avoid.
    pub fn safe_area_insets(&self) -> (f32, f32, f32, f32) {
        if self.view.is_null() {
            return (0.0, 0.0, 0.0, 0.0);
        }
        unsafe {
            // UIEdgeInsets { top, left, bottom, right } — all CGFloat
            #[repr(C)]
            #[derive(Debug, Clone, Copy)]
            struct UIEdgeInsets {
                top: f64,
                left: f64,
                bottom: f64,
                right: f64,
            }

            unsafe impl Encode for UIEdgeInsets {
                const ENCODING: Encoding = Encoding::Struct(
                    "UIEdgeInsets",
                    &[
                        Encoding::Double,
                        Encoding::Double,
                        Encoding::Double,
                        Encoding::Double,
                    ],
                );
            }

            unsafe impl RefEncode for UIEdgeInsets {
                const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
            }

            let insets: UIEdgeInsets = msg_send![self.view, safeAreaInsets];
            (
                insets.top as f32,
                insets.bottom as f32,
                insets.left as f32,
                insets.right as f32,
            )
        }
    }

    /// Advance the momentum scroller by one frame and emit a synthetic
    /// `ScrollWheel` event if the fling is still active.
    ///
    /// Called from `gpui_ios_request_frame` on every CADisplayLink tick,
    /// **before** the GPUI render callback runs, so that the scroll delta
    /// is picked up during the current frame's layout/paint cycle.
    pub(crate) fn pump_momentum(&self) {
        if crate::mobile_events::raw_touch_only() {
            self.cancel_legacy_touch();
            return;
        }
        let mut scroller = self.momentum_scroller.borrow_mut();
        if !scroller.is_active() {
            return;
        }
        let delta = scroller.step();
        let ended = !scroller.is_active();
        let position = gpui::point(
            gpui::px(scroller.position_x()),
            gpui::px(scroller.position_y()),
        );
        drop(scroller);
        let modifiers = self.modifiers.get();
        if let Some(delta) = delta {
            self.dispatch_input(PlatformInput::Scroll(gpui::ScrollEvent {
                position,
                delta: gpui::ScrollDelta::Pixels(gpui::point(
                    gpui::px(delta.dx),
                    gpui::px(delta.dy),
                )),
                modifiers,
                touch_phase: gpui::TouchPhase::Moved,
                source: gpui::ScrollSource::Touch,
            }));
        }
        if ended {
            self.dispatch_input(PlatformInput::Scroll(gpui::ScrollEvent {
                position,
                delta: gpui::ScrollDelta::Pixels(Point::default()),
                modifiers,
                touch_phase: gpui::TouchPhase::Ended,
                source: gpui::ScrollSource::Touch,
            }));
            self.clear_touch_hover();
        } else {
            self.frame_demand.wake();
        }
    }

    /// Show the software keyboard with the specified keyboard type.
    ///
    /// The actual `becomeFirstResponder` call is deferred to the next run-loop
    /// iteration via `performSelector:withObject:afterDelay:` to avoid re-entering
    /// GPUI's event dispatch while an entity lease is active (UIKit's keyboard
    /// presentation can synchronously trigger layout callbacks).
    pub fn show_keyboard_with_type(&self, keyboard_type: crate::KeyboardType) {
        log::info!("GPUI iOS: Showing keyboard (type={:?})", keyboard_type);
        unsafe {
            use crate::KeyboardType;
            let kb_type: isize = match keyboard_type {
                KeyboardType::Default => 0,      // UIKeyboardTypeDefault
                KeyboardType::EmailAddress => 7, // UIKeyboardTypeEmailAddress
                KeyboardType::Phone => 5,        // UIKeyboardTypePhonePad
                KeyboardType::NumberPad => 4,    // UIKeyboardTypeNumberPad
                KeyboardType::URL => 3,          // UIKeyboardTypeURL
                KeyboardType::Decimal => 8,      // UIKeyboardTypeDecimalPad
            };
            log::info!(
                "GPUI iOS: text_input_view={:p}, setKeyboardType: {}",
                self.text_input_view,
                kb_type
            );
            if self.text_input_view.is_null() {
                log::error!("GPUI iOS: text_input_view is NULL!");
                return;
            }
            let _: () = msg_send![self.text_input_view, setKeyboardType: kb_type];
            self.apply_input_configuration();
            log::info!("GPUI iOS: scheduling becomeFirstResponder");

            // Defer becomeFirstResponder to the next run-loop iteration.
            let _: () = msg_send![self.text_input_view,
                performSelector: sel!(becomeFirstResponder),
                withObject: ptr::null::<AnyObject>(),
                afterDelay: 0.0_f64
            ];
            log::info!("GPUI iOS: show_keyboard_with_type done");
        }
    }

    /// Hide the software keyboard.
    ///
    /// Deferred to the next run-loop iteration (like `show_keyboard_with_type`)
    /// to avoid re-entering GPUI event dispatch.
    pub fn hide_keyboard(&self) {
        log::info!("GPUI iOS: Hiding keyboard");
        unsafe {
            let _: () = msg_send![self.text_input_view,
                performSelector: sel!(resignFirstResponder),
                withObject: ptr::null::<AnyObject>(),
                afterDelay: 0.0_f64
            ];
        }
    }

    /// Handle text input from the software keyboard
    pub fn handle_text_input(&self, text: *mut AnyObject) {
        if text.is_null() {
            return;
        }

        unsafe {
            // Convert NSString to Rust String
            let utf8: *const i8 = msg_send![text, UTF8String];
            if utf8.is_null() {
                return;
            }

            let text_str = std::ffi::CStr::from_ptr(utf8)
                .to_string_lossy()
                .into_owned();

            log::info!("GPUI iOS: Text input: {:?}", text_str);

            // Try the global text input callback (for our TextInput components).
            // The text is captured in PENDING_TEXT regardless of whether we also
            // send key events below.
            let dispatched = crate::dispatch_text_input(&text_str);
            if self.closed.get() {
                return;
            }

            // Try the input handler (for GPUI's built-in text fields)
            if !dispatched {
                if self
                    .input_handler
                    .call(
                        || !self.closed.get(),
                        |handler| handler.replace_text_in_range(None, &text_str),
                    )
                    .is_some()
                {
                    return;
                }
            }

            // Send key events through GPUI's input callback.
            // Even if dispatch_text_input captured the text, we still send key
            // events so GPUI triggers a re-render cycle (which runs
            // drain_pending_text and updates the UI).
            for c in text_str.chars() {
                let keystroke = gpui::Keystroke {
                    modifiers: Modifiers::default(),
                    key: c.to_string(),
                    key_char: Some(c.to_string()),
                };

                let event = PlatformInput::KeyDown(gpui::KeyDownEvent {
                    keystroke,
                    is_held: false,
                    prefer_character_input: true,
                });

                self.dispatch_input(event);
            }
        }
    }

    /// Handle the delete-backward action from the software keyboard.
    ///
    /// This is called by the `GPUITextInputView` when the user taps the
    /// backspace key.  We dispatch a special sentinel ("\x08") through the
    /// global text input callback so the active TextInput component can
    /// remove the last character.
    pub fn handle_delete_backward(&self) {
        log::info!("GPUI iOS: deleteBackward");

        // Try the global callback first (backspace = "\x08")
        crate::dispatch_text_input("\x08");

        // Always send a Backspace KeyDown event through GPUI to trigger
        // a re-render cycle (which runs drain_pending_text).
        let keystroke = gpui::Keystroke {
            modifiers: Modifiers::default(),
            key: "backspace".to_string(),
            key_char: None,
        };
        let event = PlatformInput::KeyDown(gpui::KeyDownEvent {
            keystroke,
            is_held: false,
            prefer_character_input: false,
        });
        self.dispatch_input(event);
    }

    /// Handle a key event from an external keyboard
    pub fn handle_key_event(
        &self,
        key_code: u32,
        modifier_flags: u32,
        unmodified: &str,
        characters: &str,
        is_key_down: bool,
    ) {
        use super::text_input::{
            key_code_to_key_down, key_code_to_key_up, key_code_to_string,
            modifier_flags_to_modifiers,
        };

        let key = key_code_to_string(key_code);
        let modifiers = modifier_flags_to_modifiers(modifier_flags);
        self.modifiers.set(modifiers);
        self.capslock.set(modifier_flags & (1 << 16) != 0);

        log::info!(
            "GPUI iOS: Key event - key: {:?}, modifiers: {:?}, down: {}",
            key,
            modifiers,
            is_key_down
        );

        // On key-down, dispatch cursor-movement control codes through the
        // global text input callback so TextField-based components receive them.
        if is_key_down {
            match key_code {
                0x50 => {
                    crate::dispatch_text_input("\x1b[D");
                } // Left arrow
                0x4F => {
                    crate::dispatch_text_input("\x1b[C");
                } // Right arrow
                0x4A => {
                    crate::dispatch_text_input("\x1b[H");
                } // Home
                0x4D => {
                    crate::dispatch_text_input("\x1b[F");
                } // End
                _ => {}
            }
        }

        let event = if is_key_down {
            key_code_to_key_down(key_code, modifier_flags, unmodified, characters)
        } else {
            key_code_to_key_up(key_code, modifier_flags, unmodified, characters)
        };

        self.dispatch_input(event);
    }

    /// Notify the window of active status changes (foreground/background).
    ///
    /// This is called by the FFI layer when the app transitions between
    /// foreground and background states.
    pub fn notify_active_status_change(&self, is_active: bool) {
        if self.closed.get() {
            return;
        }
        let previously_visible = self.can_render();
        if self.app_active.replace(is_active) == is_active {
            return;
        }
        if !is_active {
            self.modifiers.set(Modifiers::default());
            self.capslock.set(false);
        }
        log::info!("GPUI iOS: Window active status changed to: {}", is_active);
        let lifecycle = if is_active {
            AppLifecycle::Resumed
        } else {
            AppLifecycle::Paused
        };
        let events = self
            .mobile_events
            .borrow_mut()
            .lifecycle(lifecycle, std::time::Duration::ZERO);
        if !is_active {
            self.cancel_legacy_touch();
            self.legacy_multitouch.set(false);
        }
        self.emit_mobile_events(events);
        self.notify_visibility(previously_visible);

        self.active_status_callback
            .call(|| !self.closed.get(), |callback| callback(is_active));
        if is_active {
            self.frame_demand.clear();
            self.frame_demand.wake();
        }
    }

    /// Handle a layout change (e.g. rotation, split-screen resize).
    ///
    /// Called from `viewDidLayoutSubviews` on the GPUIViewController.
    /// Queries the current UIView bounds, updates the stored bounds/scale,
    /// reconfigures the Metal layer + wgpu surface, and fires the resize callback.
    pub fn handle_layout_change(&self) {
        self.update_tablet_keyboard_bounds();
        self.sync_surface_availability();
        self.emit_insets();
        if self.closed.get() {
            return;
        }
        unsafe {
            let view_bounds: ObjcCGRect = msg_send![self.view, bounds];
            let screen: *mut AnyObject = msg_send![class!(UIScreen), mainScreen];
            let scale: core_graphics::base::CGFloat = msg_send![screen, scale];

            let new_w = view_bounds.width as f32;
            let new_h = view_bounds.height as f32;
            let new_scale = scale as f32;

            let old_bounds = self.bounds.get();
            let old_scale = self.scale_factor.get();

            let new_size = size(px(new_w), px(new_h));

            // Only process if something actually changed.
            if old_bounds.size == new_size && (old_scale - new_scale).abs() < 0.01 {
                return;
            }

            log::info!(
                "GPUI iOS: Layout changed — {:?} @{:.1}x → {:?} @{:.1}x",
                old_bounds.size,
                old_scale,
                new_size,
                new_scale,
            );

            // Update stored bounds (in logical pixels, matching GPUI convention).
            let new_bounds = Bounds {
                origin: Default::default(),
                size: new_size,
            };
            self.bounds.set(new_bounds);
            self.scale_factor.set(new_scale);

            // Update the Metal layer's contentsScale so the drawable has the
            // correct pixel dimensions.
            let layer: *mut AnyObject = msg_send![self.view, layer];
            let _: () = msg_send![layer, setContentsScale: scale];

            // Update the wgpu renderer's surface configuration.
            let pixel_w = (new_w * new_scale) as i32;
            let pixel_h = (new_h * new_scale) as i32;
            {
                let mut guard = self.renderer.lock();
                if let Some(renderer) = guard.as_mut() {
                    renderer
                        .update_drawable_size(size(DevicePixels(pixel_w), DevicePixels(pixel_h)));
                }
            }

            // Fire the resize callback so GPUI re-layouts at the new size.
            self.resize_callback.call(
                || !self.closed.get(),
                |callback| callback(new_size, new_scale),
            );
        }
    }
}

impl Drop for IosWindow {
    fn drop(&mut self) {
        self.inner.closed.set(true);
        super::ffi::unregister_window(self.inner.handle_id.get());
        unsafe {
            for view in [self.view, self.text_input_view] {
                if !view.is_null() {
                    #[allow(deprecated)]
                    {
                        *(*view).get_mut_ivar::<*mut c_void>(GPUI_WINDOW_IVAR) = ptr::null_mut();
                    }
                }
            }
            let center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
            let observers = std::mem::take(&mut *self.keyboard_observers.borrow_mut());
            for observer in observers {
                let _: () = msg_send![center, removeObserver: observer];
            }
            let _: () = msg_send![class!(NSObject), cancelPreviousPerformRequestsWithTarget: self.text_input_view];
        }
        self.request_frame_callback.set(None);
        self.frame_demand.set_host_waker(None);
        self.input_callback.set(None);
        self.active_status_callback.set(None);
        self.visibility_callback.set(None);
        self.resize_callback.set(None);
        self.hover_status_callback.set(None);
        self.moved_callback.set(None);
        self.should_close_callback.set(None);
        self.hit_test_callback.set(None);
        self.close_callback.set(None);
        self.appearance_changed_callback.set(None);
        self.insets_changed_callback.set(None);
    }
}

impl Drop for IosWindowInner {
    fn drop(&mut self) {
        if let Some(mut renderer) = self.renderer.get_mut().take() {
            renderer.destroy();
        }
        unsafe {
            let _: () = msg_send![self.window, setHidden: true];
            let _: () = msg_send![self.window, setRootViewController: ptr::null::<AnyObject>()];
            for object in [
                self.text_input_view,
                self.view,
                self.view_controller,
                self.window,
            ] {
                let _: () = msg_send![object, release];
            }
        }
    }
}

impl HasWindowHandle for IosWindow {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
    {
        let view = NonNull::new(self.view as *mut c_void)
            .ok_or(raw_window_handle::HandleError::Unavailable)?;
        let handle = UiKitWindowHandle::new(view);
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(handle.into()) })
    }
}

impl HasDisplayHandle for IosWindow {
    fn display_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError>
    {
        let handle = UiKitDisplayHandle::new();
        Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(handle.into()) })
    }
}

impl PlatformWindow for IosWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds.get()
    }

    fn insets(&self) -> gpui::WindowInsets {
        let mobile_insets = self.mobile_insets.borrow();
        platform_insets(self.bounds.get(), &mobile_insets)
    }

    fn on_insets_changed(&self, callback: Box<dyn FnMut(gpui::WindowInsets)>) {
        self.insets_changed_callback.set(Some(callback));
    }

    fn is_maximized(&self) -> bool {
        true // iOS windows are always "maximized"
    }

    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Fullscreen(self.bounds.get())
    }

    fn content_size(&self) -> Size<Pixels> {
        self.bounds.get().size
    }

    fn resize(&mut self, _size: Size<Pixels>) {
        // iOS windows cannot be resized programmatically
    }

    fn scale_factor(&self) -> f32 {
        self.scale_factor.get()
    }

    fn appearance(&self) -> WindowAppearance {
        unsafe {
            let trait_collection: *mut AnyObject = msg_send![self.view, traitCollection];
            let style: i64 = msg_send![trait_collection, userInterfaceStyle];
            match style {
                2 => WindowAppearance::Dark,
                _ => WindowAppearance::Light,
            }
        }
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(Rc::new(IosDisplay::main()))
    }

    fn mouse_position(&self) -> Point<Pixels> {
        self.mouse_position.get()
    }

    fn modifiers(&self) -> Modifiers {
        self.modifiers.get()
    }

    fn capslock(&self) -> Capslock {
        Capslock {
            on: self.capslock.get(),
        }
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        let focus_changed = self
            .proxy_input_session
            .borrow_mut()
            .set_focus(input_handler.focus_id());
        if focus_changed {
            // A single hidden UITextView serves all fields. Never let its
            // marked text or mapping from the previous field reach this one.
            unsafe { clear_proxy_context(self.text_input_view, self) };
        }
        self.input_handler.set(Some(input_handler));
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.input_handler.take()
    }

    fn set_text_input_configuration(&mut self, configuration: gpui::TextInputConfiguration) {
        if *self.input_configuration.borrow() == configuration {
            return;
        }
        *self.input_configuration.borrow_mut() = configuration;
        if !self.closed.get() {
            self.apply_input_configuration();
        }
    }

    fn prompt(
        &self,
        _level: PromptLevel,
        msg: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
    ) -> Option<futures::channel::oneshot::Receiver<usize>> {
        if answers.is_empty() || self.closed.get() {
            return None;
        }
        let inner = self.inner.clone();
        let (tx, rx) = futures::channel::oneshot::channel();
        let sender = Rc::new(RefCell::new(Some(tx)));

        unsafe {
            // Create UIAlertController
            let title = msg;
            let message = detail.unwrap_or("");

            let alert_style: i64 = 1; // UIAlertControllerStyleAlert

            // Rust strings are not NUL-terminated; preserve their exact UTF-8 bytes.
            let title_str: *mut AnyObject = msg_send![class!(NSString), alloc];
            let title_str: *mut AnyObject = msg_send![title_str,
                initWithBytes: title.as_ptr(), length: title.len(), encoding: 4_usize];
            let message_str: *mut AnyObject = msg_send![class!(NSString), alloc];
            let message_str: *mut AnyObject = msg_send![message_str,
                initWithBytes: message.as_ptr(), length: message.len(), encoding: 4_usize];

            let alert: *mut AnyObject = msg_send![
                class!(UIAlertController),
                alertControllerWithTitle: title_str,
                message: message_str,
                preferredStyle: alert_style
            ];
            let _: () = msg_send![title_str, release];
            let _: () = msg_send![message_str, release];

            // Add buttons
            for (index, button) in answers.iter().enumerate() {
                let label = button.label();
                let label = label.as_str();
                let button_title: *mut AnyObject = msg_send![class!(NSString), alloc];
                let button_title: *mut AnyObject = msg_send![button_title,
                    initWithBytes: label.as_ptr(), length: label.len(), encoding: 4_usize];

                let action_style: i64 = if button.is_cancel() { 1 } else { 0 }; // UIAlertActionStyleCancel or Default

                let sender = sender.clone();
                let handler = block2::RcBlock::new(move |_action: *mut AnyObject| {
                    let sender = sender.borrow_mut().take();
                    if let Some(sender) = sender {
                        let _ = sender.send(index);
                    }
                });
                let action: *mut AnyObject = msg_send![
                    class!(UIAlertAction),
                    actionWithTitle: button_title,
                    style: action_style,
                    handler: &*handler
                ];
                let _: () = msg_send![button_title, release];

                let _: () = msg_send![alert, addAction: action];
            }

            // Present the alert
            let _: () = msg_send![
                inner.view_controller,
                presentViewController: alert,
                animated: true,
                completion: ptr::null::<AnyObject>()
            ];
        }

        Some(rx)
    }

    fn activate(&self) {
        let inner = self.inner.clone();
        unsafe {
            let _: () = msg_send![inner.window, makeKeyAndVisible];
        }
    }

    fn is_active(&self) -> bool {
        unsafe {
            let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
            let key_window: *mut AnyObject = msg_send![app, keyWindow];
            self.window == key_window
        }
    }

    fn visibility(&self) -> gpui::WindowVisibility {
        if self.can_render() {
            gpui::WindowVisibility::Visible
        } else {
            gpui::WindowVisibility::Hidden
        }
    }

    fn on_visibility_change(&self, callback: Box<dyn FnMut(gpui::WindowVisibility)>) {
        self.visibility_callback.set(Some(callback));
    }

    fn is_hovered(&self) -> bool {
        // Hover isn't really applicable on iOS
        false
    }

    fn set_title(&mut self, _title: &str) {
        // iOS apps don't have window titles
    }

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        WindowBackgroundAppearance::Opaque
    }

    fn set_background_appearance(&self, _background_appearance: WindowBackgroundAppearance) {
        // Could adjust view background color
    }

    fn minimize(&self) {
        // iOS apps cannot be minimized
    }

    fn zoom(&self) {
        // iOS apps cannot be zoomed
    }

    fn toggle_fullscreen(&self) {
        // iOS apps are always fullscreen
    }

    fn is_fullscreen(&self) -> bool {
        true
    }

    fn frame_waker(&self) -> Option<Rc<dyn Fn()>> {
        let demand = Rc::downgrade(&self.frame_demand);
        Some(Rc::new(move || {
            if let Some(demand) = demand.upgrade() {
                demand.wake();
            }
        }))
    }

    fn schedule_frame(&self) {
        self.frame_demand.wake();
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.request_frame_callback.set(Some(callback));
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.input_callback.set(Some(callback));
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.active_status_callback.set(Some(callback));
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.hover_status_callback.set(Some(callback));
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.resize_callback.set(Some(callback));
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.moved_callback.set(Some(callback));
    }

    fn on_display_changed(&self, _callback: Box<dyn FnMut()>) {
        // A UIKit window remains on its UIScreen for this backend's lifetime.
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.should_close_callback.set(Some(callback));
    }

    fn on_hit_test_window_control(&self, callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
        self.hit_test_callback.set(Some(callback));
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.close_callback.set(Some(callback));
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.appearance_changed_callback.set(Some(callback));
    }

    fn draw(&self, scene: &Scene) {
        if !self.can_render() {
            return;
        }
        let mut guard = self.renderer.lock();
        if let Some(renderer) = guard.as_mut() {
            renderer.draw(scene);
        } else {
            log::trace!("GPUI iOS: draw called but no renderer available");
        }
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        let guard = self.renderer.lock();
        if let Some(renderer) = guard.as_ref() {
            renderer.sprite_atlas().clone()
        } else {
            // Fallback: return a dummy atlas so GPUI doesn't panic before
            // the renderer is initialised.
            Arc::new(FallbackAtlas::new())
        }
    }

    fn is_subpixel_rendering_supported(&self) -> bool {
        let guard = self.renderer.lock();
        guard
            .as_ref()
            .map(|r| r.supports_dual_source_blending())
            .unwrap_or(false)
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        let guard = self.renderer.lock();
        guard.as_ref().and_then(|r| r.gpu_specs())
    }

    fn update_ime_position(&self, bounds: Bounds<Pixels>) {
        let x = f32::from(bounds.origin.x);
        let y = f32::from(bounds.origin.y);
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        if self.closed.get()
            || ![x, y, width, height].iter().all(|value| value.is_finite())
            || width < 0.0
            || height < 0.0
        {
            return;
        }
        unsafe {
            let frame = ObjcCGRect::new(
                x as f64,
                y as f64,
                width.max(1.0) as f64,
                height.max(1.0) as f64,
            );
            let _: () = msg_send![self.text_input_view, setFrame: frame];
        }
    }

    fn text_input_state_changed(&self, change: gpui::TextInputStateChange) {
        if self.closed.get() {
            return;
        }
        // GPUI takes/restores handlers during painting; only actual focus
        // transitions should change UIKit's first responder.
        match change {
            gpui::TextInputStateChange::FocusGained => {
                self.show_keyboard_with_type(crate::KeyboardType::Default);
            }
            gpui::TextInputStateChange::FocusLost => {
                if self.proxy_input_session.borrow_mut().set_focus(None) {
                    unsafe { clear_proxy_context(self.text_input_view, self) };
                }
                self.hide_keyboard();
            }
            gpui::TextInputStateChange::SelectionChanged
            | gpui::TextInputStateChange::ContentChanged => {}
        }
    }
}

// ── Fallback atlas ────────────────────────────────────────────────────────────

/// A minimal fallback `PlatformAtlas` used until a real Blade/Metal renderer is
/// wired up.  It records tiles in memory but does not upload texture data to the
/// GPU — just enough to satisfy GPUI's atlas queries without panicking.
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
            Option<(Size<DevicePixels>, std::borrow::Cow<'a, [u8]>)>,
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
                texture_id: AtlasTextureId {
                    index: 0,
                    kind: AtlasTextureKind::Monochrome,
                },
                tile_id: TileId(id),
                padding: 0,
                bounds: Bounds {
                    origin: point(DevicePixels(0), DevicePixels(0)),
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

fn step_pinch_delta(last: &Cell<Option<f32>>, phase: gpui::TouchPhase, scale: f32) -> Option<f32> {
    match phase {
        gpui::TouchPhase::Started => {
            if !scale.is_finite() || scale <= 0.0 {
                return None;
            }
            last.set(Some(scale));
            Some(0.0)
        }
        gpui::TouchPhase::Moved => {
            if !scale.is_finite() || scale <= 0.0 {
                return None;
            }
            let prev = last.get()?.max(1e-4);
            last.set(Some(scale));
            Some((scale / prev).clamp(0.2, 5.0).ln())
        }
        gpui::TouchPhase::Ended | gpui::TouchPhase::Cancelled => last.take().map(|_| 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::{appearance_style_changed, edge_attached_keyboard_insets, ProxyInputSession};

    #[test]
    fn appearance_callback_requires_a_real_style_transition() {
        assert!(!appearance_style_changed(None, Some(1)));
        assert!(!appearance_style_changed(Some(1), Some(1)));
        assert!(appearance_style_changed(Some(1), Some(2)));
    }

    #[test]
    fn composition_from_a_is_rejected_after_a_to_b_to_a_focus_switch() {
        let mut proxy = ProxyInputSession::new();

        assert!(proxy.set_focus(Some("A")));
        let a_composition = proxy.generation();
        // Repainting field A must not discard its active Korean composition.
        assert!(!proxy.set_focus(Some("A")));
        assert!(proxy.is_current(a_composition));

        assert!(proxy.set_focus(Some("B")));
        let b_composition = proxy.generation();
        assert!(!proxy.is_current(a_composition));

        assert!(proxy.set_focus(Some("A")));
        let returned_to_a = proxy.generation();
        assert!(!proxy.is_current(a_composition));
        assert!(!proxy.is_current(b_composition));
        assert!(proxy.is_current(returned_to_a));

        assert!(proxy.set_focus(None));
        assert!(!proxy.is_current(returned_to_a));
    }

    #[test]
    fn platform_insets_ignore_floating_keyboard_geometry() {
        let window = gpui::Bounds {
            origin: gpui::Point::default(),
            size: gpui::size(gpui::px(400.0), gpui::px(800.0)),
        };
        let floating = gpui::Bounds {
            origin: gpui::point(gpui::px(100.0), gpui::px(300.0)),
            size: gpui::size(gpui::px(200.0), gpui::px(200.0)),
        };
        assert_eq!(
            edge_attached_keyboard_insets(window, Some(floating)),
            gpui::Edges::default()
        );

        let docked = gpui::Bounds {
            origin: gpui::point(gpui::px(0.0), gpui::px(500.0)),
            size: gpui::size(gpui::px(400.0), gpui::px(300.0)),
        };
        assert_eq!(
            edge_attached_keyboard_insets(window, Some(docked)).bottom,
            gpui::px(300.0)
        );
    }

    #[test]
    fn step_pinch_delta_tracks_incremental_log_scale() {
        let last = Cell::new(None);
        assert_eq!(step_pinch_delta(&last, gpui::TouchPhase::Moved, 1.5), None);
        assert_eq!(
            step_pinch_delta(&last, gpui::TouchPhase::Started, 1.0),
            Some(0.0)
        );
        let d1 = step_pinch_delta(&last, gpui::TouchPhase::Moved, 1.25).unwrap();
        assert!((d1.exp() - 1.25).abs() < 1e-5);
        let d2 = step_pinch_delta(&last, gpui::TouchPhase::Moved, 1.5).unwrap();
        assert!((d2.exp() - 1.2).abs() < 1e-5);
        assert_eq!(
            step_pinch_delta(&last, gpui::TouchPhase::Ended, 1.5),
            Some(0.0)
        );
        assert_eq!(last.get(), None);
    }
}
