//! VoiceOver support: exposes GPUI's AccessKit tree through UIKit.
//!
//! GPUI builds an AccessKit tree while a screen reader is active and passes it
//! to `PlatformWindow::a11y_tree_update`. `accesskit_ios` turns that tree into
//! virtual `UIAccessibilityElement`s on the Metal view, so GPUI controls keep
//! their labels, roles, focus and actions without a parallel UIKit view tree.

use std::ffi::c_void;

use accesskit_ios::{Adapter, CGPoint, QueuedEvents};
use gpui::{
    accesskit::{ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate},
    A11yCallbacks,
};
use std::sync::{Arc, Mutex};

/// One window's GPUI callbacks, shared by the adapter and its observers.
#[derive(Clone)]
pub(super) struct Handlers(Arc<Mutex<A11yCallbacks>>);

impl Handlers {
    fn new(callbacks: A11yCallbacks) -> Self {
        Self(Arc::new(Mutex::new(callbacks)))
    }
}

impl ActivationHandler for Handlers {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        (self.0.lock().expect("poisoned").activation)()
    }
}

impl ActionHandler for Handlers {
    fn do_action(&mut self, request: ActionRequest) {
        (self.0.lock().expect("poisoned").action)(request)
    }
}

impl DeactivationHandler for Handlers {
    fn deactivate_accessibility(&mut self) {
        (self.0.lock().expect("poisoned").deactivation)()
    }
}

/// The adapter answers UIKit queries through `GPUIMetalView`'s overrides.
pub(super) struct Accessibility {
    adapter: Adapter,
}

impl Accessibility {
    /// Attach to the window's root view. The view class already forwards the
    /// accessibility methods, so attaching after the view appears is safe.
    ///
    /// # Safety
    /// `view` must be a live `UIView` on the main thread.
    pub(super) unsafe fn new(view: *mut c_void, callbacks: A11yCallbacks) -> Self {
        let handlers = Handlers::new(callbacks);
        let adapter = unsafe { Adapter::new(view, handlers.clone(), handlers.clone(), handlers) };
        Self { adapter }
    }

    /// Apply a tree update and let VoiceOver announce the resulting changes.
    pub(super) fn update(&self, tree: TreeUpdate) {
        if let Some(events) = self.adapter.update_if_active(|| tree) {
            events.raise();
        }
    }

    pub(super) fn is_accessibility_element(&self) -> bool {
        self.adapter.is_accessibility_element()
    }

    pub(super) fn accessibility_elements(&self) -> *mut objc2::runtime::AnyObject {
        self.adapter.accessibility_elements() as *mut objc2::runtime::AnyObject
    }

    pub(super) fn hit_test(&self, x: f64, y: f64) -> *mut objc2::runtime::AnyObject {
        self.adapter.hit_test(CGPoint { x, y }) as *mut objc2::runtime::AnyObject
    }

    pub(super) fn view_did_appear(&self) -> Option<QueuedEvents> {
        self.adapter.view_did_appear()
    }
}

/// AccessKit's UIKit adapter requires main-thread calls.
pub(super) fn is_main_thread() -> bool {
    unsafe {
        let result: objc2::runtime::Bool = objc2::msg_send![objc2::class!(NSThread), isMainThread];
        result.as_bool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn callbacks(counts: Arc<[AtomicUsize; 3]>) -> A11yCallbacks {
        A11yCallbacks {
            activation: Box::new({
                let counts = counts.clone();
                move || {
                    counts[0].fetch_add(1, Ordering::SeqCst);
                    None
                }
            }),
            action: Box::new({
                let counts = counts.clone();
                move |_| {
                    counts[1].fetch_add(1, Ordering::SeqCst);
                }
            }),
            deactivation: Box::new({
                let counts = counts.clone();
                move || {
                    counts[2].fetch_add(1, Ordering::SeqCst);
                }
            }),
        }
    }

    #[test]
    fn handlers_forward_activation_action_and_deactivation() {
        let counts = Arc::new([
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
        ]);
        let mut handlers = Handlers::new(callbacks(counts.clone()));

        assert!(handlers.request_initial_tree().is_none());
        handlers.do_action(ActionRequest {
            action: gpui::accesskit::Action::Focus,
            target_tree: gpui::accesskit::TreeId::ROOT,
            target_node: gpui::accesskit::NodeId(1),
            data: None,
        });
        handlers.deactivate_accessibility();

        assert_eq!(counts[0].load(Ordering::SeqCst), 1);
        assert_eq!(counts[1].load(Ordering::SeqCst), 1);
        assert_eq!(counts[2].load(Ordering::SeqCst), 1);
    }
}
