//! Window-local OS facts. Gesture recognition, click synthesis and layout belong to the app.
//!
//! Feed these events on the window's UI thread, in OS delivery order. Coordinates are
//! window-local logical pixels (UIKit points; Android physical pixels / density).

use gpui::{Bounds, Edges, Pixels, Point, TouchEvent, TouchId, TouchPhase};
use std::{collections::BTreeMap, time::Duration};

type EventHandler = Box<dyn FnMut(u64, &MobileEvent)>;

/// Thread-wide policy, shared by all windows on the current UI thread. RawOnly
/// gives the app sole ownership of gesture recognition and mouse/scroll synthesis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TouchDeliveryPolicy {
    #[default]
    LegacyCompatible,
    RawOnly,
}

thread_local! {
    static TOUCH_DELIVERY: std::cell::Cell<TouchDeliveryPolicy> = const {
        std::cell::Cell::new(TouchDeliveryPolicy::LegacyCompatible)
    };
}

/// May be selected synchronously in the first Started observer before synthesis.
/// Keep it latched through all terminal events until the next gesture's Started.
/// Do not pump nested native input while choosing a gesture's delivery policy.
pub fn set_touch_delivery_policy(policy: TouchDeliveryPolicy) {
    TOUCH_DELIVERY.set(policy);
}

pub(crate) fn raw_touch_only() -> bool {
    TOUCH_DELIVERY.get() == TouchDeliveryPolicy::RawOnly
}

thread_local! {
    static EVENTS: crate::mobile_dispatch::EventQueue<(u64, MobileEvent)> = Default::default();
    static APP_LIFECYCLE: crate::mobile_dispatch::EventQueue<gpui::AppLifecyclePhase> = Default::default();
    static MEMORY_WARNING: crate::mobile_dispatch::EventQueue<()> = Default::default();
    static OPEN_URLS: crate::mobile_dispatch::EventQueue<Vec<String>> = Default::default();
    static THERMAL_CHANGE: crate::mobile_dispatch::EventQueue<()> = Default::default();
}

pub(crate) fn set_app_lifecycle_handler(mut callback: Box<dyn FnMut(gpui::AppLifecyclePhase)>) {
    APP_LIFECYCLE.with(|queue| queue.set_handler(Some(Box::new(move |phase| callback(*phase)))));
}
pub(crate) fn dispatch_app_lifecycle(phase: gpui::AppLifecyclePhase) {
    APP_LIFECYCLE.with(|queue| queue.dispatch([phase]));
}
pub(crate) fn set_memory_warning_handler(mut callback: Box<dyn FnMut()>) {
    MEMORY_WARNING.with(|queue| queue.set_handler(Some(Box::new(move |_| callback()))));
}
pub(crate) fn dispatch_memory_warning() {
    MEMORY_WARNING.with(|queue| queue.dispatch([()]));
}
pub(crate) fn set_open_urls_handler(mut callback: Box<dyn FnMut(Vec<String>)>) {
    OPEN_URLS.with(|queue| queue.set_handler(Some(Box::new(move |urls| callback(urls.clone())))));
}
pub(crate) fn dispatch_open_urls(urls: Vec<String>) {
    OPEN_URLS.with(|queue| queue.dispatch([urls]));
}
pub(crate) fn set_thermal_change_handler(mut callback: Box<dyn FnMut()>) {
    THERMAL_CHANGE.with(|queue| queue.set_handler(Some(Box::new(move |_| callback()))));
}
pub(crate) fn dispatch_thermal_change() {
    THERMAL_CHANGE.with(|queue| queue.dispatch([()]));
}

/// Register on the platform UI thread. The first argument identifies the window.
/// Reentrant events are queued after the current batch; replacing the observer is safe.
/// It observes OS facts; it does not consume the existing GPUI tap/scroll path.
pub fn set_mobile_event_handler(handler: Option<EventHandler>) {
    EVENTS.with(|queue| {
        queue.set_handler(handler.map(|mut handler| {
            Box::new(move |event: &(u64, MobileEvent)| handler(event.0, &event.1))
                as Box<dyn FnMut(&(u64, MobileEvent))>
        }))
    });
}

pub(crate) fn dispatch(window: u64, event: &MobileEvent) {
    dispatch_batch(window, [event.clone()]);
}

pub(crate) fn dispatch_batch(window: u64, events: impl IntoIterator<Item = MobileEvent>) {
    EVENTS.with(|queue| queue.dispatch(events.into_iter().map(|event| (window, event))));
}

/// Insets are independent observations: consumers must not blindly add safe area and IME.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowInsets {
    pub safe_area: Edges<Pixels>,
    /// Reported keyboard intersection, including floating keyboards. None also covers
    /// platforms without an IME geometry feed; see docs/mobile-events.md.
    pub keyboard: Option<Bounds<Pixels>>,
    /// None means the platform has not reported visibility, not that IME is hidden.
    pub keyboard_visible: Option<bool>,
    /// Independent Android IME edge occlusion; never add this to safe_area blindly.
    pub keyboard_insets: Option<Edges<Pixels>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppLifecycle {
    Resumed,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceLifecycle {
    /// A new drawable is available. Recreate GPU resources before rendering.
    Available,
    /// The drawable is no longer usable. Stop rendering and release its resources.
    Lost,
}

#[derive(Clone, Debug)]
pub enum MobileEvent {
    Touch {
        event: TouchEvent,
        timestamp: Duration,
    },
    /// UIKit recognizer or Android two-pointer span. Scale is cumulative, not a delta.
    /// App code must choose this stream or recognition from raw touches, never both.
    Pinch {
        phase: TouchPhase,
        center: Point<Pixels>,
        scale: f32,
        timestamp: Duration,
    },
    Insets(WindowInsets),
    Lifecycle(AppLifecycle),
    Surface(SurfaceLifecycle),
}

/// Per-window pointer bookkeeping. Native IDs may be reused; GPUI touch IDs may not.
/// Cancellation never emits Ended or mouse events. Drain events before calling consumers
/// so callbacks can safely re-enter the platform without borrowing this state.
#[derive(Default)]
pub struct MobileEventState {
    pointers: BTreeMap<u64, TouchEvent>,
    next_id: u64,
    paused: bool,
    surface_available: bool,
    pinch: Option<(Point<Pixels>, f32)>,
    pinch_span: Option<f32>,
}

impl MobileEventState {
    pub fn active_pointer_count(&self) -> usize {
        self.pointers.len()
    }
    /// Ignore stale phases and invalid positions at the OS boundary. Duplicate down is
    /// ignored rather than silently replacing a live pointer without cancellation.
    pub fn touch(
        &mut self,
        native_id: u64,
        phase: TouchPhase,
        position: Point<Pixels>,
        timestamp: Duration,
    ) -> Option<MobileEvent> {
        if self.paused || !self.surface_available {
            return None;
        }
        let position = if finite(position) {
            position
        } else {
            match phase {
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    self.pointers.get(&native_id)?.position
                }
                _ => return None,
            }
        };
        let event = match phase {
            TouchPhase::Started => {
                if self.pointers.contains_key(&native_id) {
                    return None;
                }
                let id = TouchId(self.next_id);
                self.next_id = self.next_id.checked_add(1)?;
                TouchEvent {
                    id,
                    phase,
                    position,
                    ..Default::default()
                }
            }
            _ => {
                let previous = self.pointers.get(&native_id)?;
                TouchEvent {
                    phase,
                    position,
                    ..previous.clone()
                }
            }
        };
        match phase {
            TouchPhase::Started | TouchPhase::Moved => {
                self.pointers.insert(native_id, event.clone());
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.pointers.remove(&native_id);
            }
        }
        Some(MobileEvent::Touch { event, timestamp })
    }

    pub fn pinch(
        &mut self,
        phase: TouchPhase,
        center: Point<Pixels>,
        scale: f32,
        timestamp: Duration,
    ) -> Option<MobileEvent> {
        if self.paused || !self.surface_available {
            return None;
        }
        let (center, scale) = if finite(center) && scale.is_finite() && scale > 0.0 {
            (center, scale)
        } else {
            match phase {
                TouchPhase::Ended | TouchPhase::Cancelled => self.pinch?,
                _ => return None,
            }
        };
        match phase {
            TouchPhase::Started if self.pinch.is_none() => self.pinch = Some((center, scale)),
            TouchPhase::Moved if self.pinch.is_some() => self.pinch = Some((center, scale)),
            TouchPhase::Ended | TouchPhase::Cancelled if self.pinch.is_some() => self.pinch = None,
            _ => return None,
        }
        Some(MobileEvent::Pinch {
            phase,
            center,
            scale,
            timestamp,
        })
    }

    /// Used for OS-wide cancellation (Android ACTION_CANCEL), pause and surface loss.
    pub fn cancel_all(&mut self, timestamp: Duration) -> Vec<MobileEvent> {
        self.pinch_span = None;
        let mut events: Vec<_> = std::mem::take(&mut self.pointers)
            .into_values()
            .map(|event| MobileEvent::Touch {
                event: TouchEvent {
                    phase: TouchPhase::Cancelled,
                    ..event
                },
                timestamp,
            })
            .collect();
        if let Some((center, scale)) = self.pinch.take() {
            events.push(MobileEvent::Pinch {
                phase: TouchPhase::Cancelled,
                center,
                scale,
                timestamp,
            });
        }
        events
    }

    /// Android has no NDK pinch recognizer. Report the two-contact span without
    /// slop, zoom limits, hit-testing or capture policy. A third contact cancels it.
    pub fn pointer_span(&mut self, timestamp: Duration) -> Option<MobileEvent> {
        if self.pointers.len() != 2 {
            self.pinch_span = None;
            let (center, scale) = self.pinch?;
            let phase = if self.pointers.len() > 2 {
                TouchPhase::Cancelled
            } else {
                TouchPhase::Ended
            };
            return self.pinch(phase, center, scale, timestamp);
        }
        let mut pointers = self.pointers.values();
        let a = pointers.next()?.position;
        let b = pointers.next()?.position;
        let dx = f32::from(a.x) - f32::from(b.x);
        let dy = f32::from(a.y) - f32::from(b.y);
        let span = dx.hypot(dy);
        if !span.is_finite() || span <= 0.0 {
            return None;
        }
        let center = gpui::point(
            gpui::px(f32::from(a.x) / 2.0 + f32::from(b.x) / 2.0),
            gpui::px(f32::from(a.y) / 2.0 + f32::from(b.y) / 2.0),
        );
        let phase = if self.pinch_span.is_none() {
            TouchPhase::Started
        } else {
            TouchPhase::Moved
        };
        let initial = *self.pinch_span.get_or_insert(span);
        self.pinch(phase, center, span / initial, timestamp)
    }

    /// Cancellation is delivered before Paused, including when the OS omits touch cancels.
    pub fn lifecycle(&mut self, lifecycle: AppLifecycle, timestamp: Duration) -> Vec<MobileEvent> {
        let paused = lifecycle == AppLifecycle::Paused;
        if self.paused == paused {
            return Vec::new();
        }
        self.paused = paused;
        let mut events = if paused {
            self.cancel_all(timestamp)
        } else {
            Vec::new()
        };
        events.push(MobileEvent::Lifecycle(lifecycle));
        events
    }

    /// Pause and drawable availability are independent. Resume alone cannot restore a surface.
    /// Deliver Lost before replacing an available surface, even if its dimensions are unchanged.
    pub fn surface(
        &mut self,
        lifecycle: SurfaceLifecycle,
        timestamp: Duration,
    ) -> Vec<MobileEvent> {
        let available = lifecycle == SurfaceLifecycle::Available;
        if self.surface_available == available {
            return Vec::new();
        }
        self.surface_available = available;
        let mut events = if available {
            Vec::new()
        } else {
            self.cancel_all(timestamp)
        };
        events.push(MobileEvent::Surface(lifecycle));
        events
    }

    pub fn can_render(&self) -> bool {
        !self.paused && self.surface_available
    }
}

fn finite(position: Point<Pixels>) -> bool {
    f32::from(position.x).is_finite() && f32::from(position.y).is_finite()
}

/// Actual window/entity identities for an opt-in editor (native GPUI thread only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextInputOwner {
    pub window: u64,
    pub editor: u64,
    pub activation: u64,
}

/// One focus activation. Release this exact lease on blur/disposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextInputLease {
    owner: TextInputOwner,
    generation: u64,
}

#[derive(Default)]
struct InputOwners {
    generation: u64,
    active: Option<TextInputLease>,
}
thread_local! {
    static INPUT_OWNERS: std::cell::RefCell<InputOwners> = Default::default();
    static OWNER_QUERIES: std::cell::RefCell<Vec<(Option<TextInputOwner>, bool)>> = Default::default();
}

/// Opt in on an actual focus transition, never on guessed caret/config changes.
pub fn activate_text_input_owner(owner: TextInputOwner) -> TextInputLease {
    let (lease, changed) = INPUT_OWNERS.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(lease) = state.active.filter(|lease| lease.owner == owner) {
            return (lease, false);
        }
        state.generation = state
            .generation
            .checked_add(1)
            .expect("IME activation exhausted");
        let lease = TextInputLease {
            owner,
            generation: state.generation,
        };
        state.active = Some(lease);
        (lease, true)
    });
    if changed {
        #[cfg(target_os = "android")]
        crate::android::jni::logical_input_owner_changed(true);
    }
    lease
}

/// Compare-and-clear: an old blur/disposal must not invalidate a newer activation.
pub fn release_text_input_owner(lease: TextInputLease) {
    let changed = INPUT_OWNERS.with(|state| {
        let mut state = state.borrow_mut();
        if state.active != Some(lease) {
            return false;
        }
        state.active = None;
        true
    });
    if changed {
        #[cfg(target_os = "android")]
        crate::android::jni::logical_input_owner_changed(false);
    }
}

/// Report from the actual handler's accepts_text_input implementation.
/// Ignored outside a backend scoped identity query; never changes focus ownership.
pub fn report_text_input_owner(owner: TextInputOwner, focused: bool, disabled: bool) {
    OWNER_QUERIES.with(|queries| {
        if let Some(slot) = queries.borrow_mut().last_mut() {
            if !focused || disabled {
                slot.1 = true;
                return;
            }
            if slot.0.is_some_and(|previous| previous != owner) {
                slot.1 = true;
            }
            slot.0 = Some(owner);
        }
    });
}

pub(crate) fn active_text_input_lease() -> Option<TextInputLease> {
    INPUT_OWNERS.with(|state| state.borrow().active)
}
impl TextInputLease {
    pub(crate) fn window(self) -> u64 {
        self.owner.window
    }
}

pub(crate) fn bind_queried_text_input_owner(
    window: u64,
    accepts: bool,
    owner: Option<TextInputOwner>,
) {
    if let Some(owner) = owner.filter(|owner| accepts && owner.window == window) {
        activate_text_input_owner(owner);
    } else if let Some(lease) = active_text_input_lease().filter(|lease| lease.window() == window) {
        release_text_input_owner(lease);
        // Transition to an uninstrumented accepting handler preserves legacy behavior.
        // Old explicit traffic is already invalidated by the release/session change.
        #[cfg(target_os = "android")]
        if accepts && owner.is_none() {
            crate::android::jni::show_keyboard_android(crate::KeyboardType::Default);
        }
    }
}

/// Query the selected handler without holding any backend state borrow.
pub(crate) fn query_text_input_owner(
    query: impl FnOnce() -> bool,
) -> (bool, Option<TextInputOwner>) {
    struct Scope;
    impl Drop for Scope {
        fn drop(&mut self) {
            OWNER_QUERIES.with(|queries| {
                queries.borrow_mut().pop();
            });
        }
    }
    OWNER_QUERIES.with(|queries| queries.borrow_mut().push((None, false)));
    let scope = Scope;
    let accepts = query();
    let (owner, invalid) = OWNER_QUERIES.with(|queries| *queries.borrow().last().unwrap());
    drop(scope);
    (accepts && !invalid, if invalid { None } else { owner })
}

pub(crate) fn queried_owner_matches(lease: TextInputLease, owner: Option<TextInputOwner>) -> bool {
    active_text_input_lease() == Some(lease) && owner == Some(lease.owner)
}

/// Gate the complete native event, not just text, against the retained handler.
pub(crate) fn verify_owned_ime(
    lease: TextInputLease,
    input: ImeInput,
    query: impl FnOnce() -> bool,
) -> Option<ImeInput> {
    let (accepts, owner) = query_text_input_owner(query);
    (accepts && queried_owner_matches(lease, owner)).then_some(input)
}

#[cfg(test)]
mod owner_tests {
    use super::*;
    fn owner(activation: u64) -> TextInputOwner {
        TextInputOwner {
            window: 7,
            editor: 11,
            activation,
        }
    }
    #[test]
    fn selected_handler_same_owner_repaint_preserves_delivery_lease() {
        let a = owner(1);
        let lease = activate_text_input_owner(a);
        for _ in 0..4 {
            let (accepts, identity) = query_text_input_owner(|| {
                report_text_input_owner(a, true, false);
                true
            });
            bind_queried_text_input_owner(7, accepts, identity);
            assert_eq!(active_text_input_lease(), Some(lease));
            assert!(queried_owner_matches(lease, identity));
        }
        release_text_input_owner(lease);
    }
    #[test]
    fn observed_a_b_a_rejects_old_commit_finish_and_release() {
        let old = activate_text_input_owner(owner(1));
        let b = activate_text_input_owner(TextInputOwner {
            editor: 12,
            ..owner(1)
        });
        let fresh = activate_text_input_owner(owner(3));
        assert_ne!(old, fresh);
        assert!(!queried_owner_matches(old, Some(owner(3))));
        assert!(!queried_owner_matches(b, Some(owner(3))));
        release_text_input_owner(old);
        release_text_input_owner(b);
        assert_eq!(active_text_input_lease(), Some(fresh));
        release_text_input_owner(fresh);
    }
    #[test]
    fn query_absent_conflicting_or_disabled_identity_never_inherits_owner() {
        let a = owner(1);
        report_text_input_owner(a, true, false);
        assert_eq!(query_text_input_owner(|| true), (true, None));
        assert_eq!(
            query_text_input_owner(|| {
                report_text_input_owner(a, true, false);
                report_text_input_owner(owner(2), true, false);
                true
            }),
            (false, None)
        );
        assert_eq!(
            query_text_input_owner(|| {
                report_text_input_owner(a, true, true);
                true
            }),
            (false, None)
        );
    }
    #[test]
    fn nested_and_unwound_queries_do_not_publish_into_outer_scope() {
        let a = owner(1);
        let result = query_text_input_owner(|| {
            report_text_input_owner(a, true, false);
            assert_eq!(
                query_text_input_owner(|| {
                    report_text_input_owner(owner(2), true, false);
                    true
                })
                .1,
                Some(owner(2))
            );
            let _ = std::panic::catch_unwind(|| {
                query_text_input_owner(|| {
                    report_text_input_owner(owner(3), true, false);
                    panic!("probe failed")
                })
            });
            true
        });
        assert_eq!(result, (true, Some(a)));
        assert_eq!(query_text_input_owner(|| true), (true, None));
    }
    #[test]
    fn unidentified_selected_handler_and_foreign_window_reject_old_owner() {
        let lease = activate_text_input_owner(owner(1));
        assert!(!queried_owner_matches(
            lease,
            Some(TextInputOwner {
                editor: 12,
                ..owner(1)
            })
        ));
        bind_queried_text_input_owner(8, true, Some(owner(1)));
        assert_eq!(active_text_input_lease(), Some(lease));
        bind_queried_text_input_owner(7, false, None);
        assert_eq!(active_text_input_lease(), None);
        assert!(!queried_owner_matches(lease, Some(owner(1))));
    }
    #[test]
    fn composing_commit_finish_survive_repaint_but_not_reactivation() {
        let a = owner(1);
        let lease = activate_text_input_owner(a);
        let events = [
            ImeInput::Compose {
                text: "ㅎ".into(),
                selection: 1..1,
            },
            ImeInput::Compose {
                text: "한".into(),
                selection: 1..1,
            },
            ImeInput::Commit("한".into()),
            ImeInput::FinishComposition,
        ];
        for event in events.iter().cloned() {
            bind_queried_text_input_owner(7, true, Some(a));
            let delivered = verify_owned_ime(lease, event, || {
                report_text_input_owner(a, true, false);
                true
            })
            .expect("same owner event retained across paint");
            if let ImeInput::Compose { text, .. } = delivered {
                assert!(!text.is_empty());
            }
        }
        let b = activate_text_input_owner(TextInputOwner { editor: 12, ..a });
        let fresh = activate_text_input_owner(owner(3));
        for event in events {
            assert!(verify_owned_ime(lease, event, || {
                report_text_input_owner(owner(3), true, false);
                true
            })
            .is_none());
        }
        release_text_input_owner(b);
        release_text_input_owner(lease);
        assert_eq!(active_text_input_lease(), Some(fresh));
        release_text_input_owner(fresh);
    }
    #[test]
    fn jni_empty_commit_survives_conversion_and_owner_gate_but_not_focus_switch() {
        let a = owner(1);
        let lease = activate_text_input_owner(a);
        let compose = verify_owned_ime(
            lease,
            ImeInput::Compose {
                text: "한".into(),
                selection: 1..1,
            },
            || {
                report_text_input_owner(a, true, false);
                true
            },
        );
        assert!(matches!(compose, Some(ImeInput::Compose { .. })));
        let empty = ime_action(0, String::new()).expect("JNI must retain empty replacement");
        let delivered = verify_owned_ime(lease, empty.clone(), || {
            report_text_input_owner(a, true, false);
            true
        });
        assert!(matches!(delivered, Some(ImeInput::Commit(text)) if text.is_empty()));
        let b_owner = TextInputOwner { editor: 12, ..a };
        let b = activate_text_input_owner(b_owner);
        assert!(verify_owned_ime(lease, empty, || {
            report_text_input_owner(b_owner, true, false);
            true
        })
        .is_none());
        assert!(ime_action(3, String::new()).is_none());
        release_text_input_owner(b);
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ImeInput {
    Commit(String),
    Compose {
        text: String,
        selection: std::ops::Range<usize>,
    },
    FinishComposition,
    Enter,
    Backspace,
    DeleteBefore {
        count: usize,
        codepoints: bool,
    },
}

/// JNI action conversion. Empty commits replace (and can delete) marked text.
pub(crate) fn ime_action(action: i32, text: String) -> Option<ImeInput> {
    match action {
        0 => Some(ImeInput::Commit(text)),
        1 => Some(ImeInput::Enter),
        2 => Some(ImeInput::Backspace),
        4 => Some(ImeInput::FinishComposition),
        _ => None,
    }
}

pub(crate) fn ime_delete_request(
    count: usize,
    codepoints: bool,
    has_handler: bool,
    legacy: bool,
) -> Option<ImeInput> {
    if has_handler {
        Some(ImeInput::DeleteBefore { count, codepoints })
    } else if count == 1 && legacy {
        // Compatibility command only; the string callback exposes no range units.
        Some(ImeInput::Backspace)
    } else {
        None
    }
}

/// Return a UTF-16 suffix length, refusing a split surrogate or insufficient context.
pub(crate) fn ime_delete_units(text: &str, count: usize, codepoints: bool) -> Option<usize> {
    if count == 0 {
        return Some(0);
    }
    let mut units = 0;
    for (index, character) in text.chars().rev().enumerate() {
        units += character.len_utf16();
        if codepoints && index + 1 == count || !codepoints && units == count {
            return Some(units);
        }
        if !codepoints && units > count {
            return None;
        }
    }
    None
}

pub(crate) fn android_input_traits(configuration: &gpui::TextInputConfiguration) -> (i32, i32) {
    use gpui::{Autocapitalize as C, TextInputAction as A};
    let mut flags = match configuration.autocapitalize {
        C::None => 0,
        C::Characters => 0x1000,
        C::Words => 0x2000,
        C::Sentences => 0x4000,
    };
    if configuration.autocorrect {
        flags |= 0x8000;
    }
    if !configuration.suggestions {
        flags |= 0x80000;
    }
    let action = match configuration.input_action {
        A::Unspecified => 0,
        A::Enter => 1,
        A::Go => 2,
        A::Search => 3,
        A::Send => 4,
        A::Next => 5,
        A::Done => 6,
        A::Previous => 7,
    };
    (flags, action | 0x10000000)
}

pub(crate) fn ios_input_traits(
    configuration: &gpui::TextInputConfiguration,
) -> (isize, isize, isize, isize) {
    use gpui::{Autocapitalize as C, TextInputAction as A};
    let capitalize = match configuration.autocapitalize {
        C::None => 0,
        C::Words => 1,
        C::Sentences => 2,
        C::Characters => 3,
    };
    let action = match configuration.input_action {
        A::Unspecified | A::Enter => 0,
        A::Go => 1,
        A::Next => 4,
        A::Search => 6,
        A::Send => 7,
        A::Done => 9,
        A::Previous => 0,
    };
    (
        if configuration.autocorrect { 2 } else { 1 },
        capitalize,
        if configuration.suggestions { 2 } else { 1 },
        action,
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn ime_deletion_preserves_utf16_and_codepoint_boundaries() {
        // Same request count, different native units: an emoji occupies two
        // UTF-16 units but one Unicode scalar.
        assert_eq!(super::ime_delete_units("a🦀", 1, true), Some(2));
        assert_eq!(super::ime_delete_units("a🦀", 1, false), None);
        assert_eq!(super::ime_delete_units("a🦀", 2, false), Some(2));
        assert_eq!(super::ime_delete_units("a🦀", 2, true), Some(3));
        // Do not silently treat combining sequences as one code point.
        assert_eq!(super::ime_delete_units("e\u{301}", 1, true), Some(1));
        assert_eq!(super::ime_delete_units("a", 2, true), None);
        assert_eq!(super::ime_delete_units("", 0, false), Some(0));
    }

    use super::*;
    use gpui::{point, px};
    #[test]
    fn platform_callbacks_allow_nested_delivery_and_replacement() {
        use std::{cell::RefCell, rc::Rc};
        let observed = Rc::new(RefCell::new(Vec::new()));
        let output = observed.clone();
        set_app_lifecycle_handler(Box::new(move |phase| {
            output.borrow_mut().push(phase);
            if phase == gpui::AppLifecyclePhase::Inactive {
                dispatch_app_lifecycle(gpui::AppLifecyclePhase::Background);
            }
        }));
        dispatch_app_lifecycle(gpui::AppLifecyclePhase::Inactive);
        assert_eq!(
            &*observed.borrow(),
            &[
                gpui::AppLifecyclePhase::Inactive,
                gpui::AppLifecyclePhase::Background
            ]
        );
        let count = Rc::new(std::cell::Cell::new(0));
        let output = count.clone();
        set_memory_warning_handler(Box::new(move || {
            output.set(output.get() + 1);
            set_memory_warning_handler(Box::new(|| {}));
            dispatch_memory_warning();
        }));
        dispatch_memory_warning();
        assert_eq!(count.get(), 1);
        let urls = Rc::new(RefCell::new(Vec::new()));
        let output = urls.clone();
        set_open_urls_handler(Box::new(move |received| {
            output.borrow_mut().extend(received)
        }));
        dispatch_open_urls(vec!["gpui://test?q=한글".to_owned()]);
        assert_eq!(&*urls.borrow(), &["gpui://test?q=한글"]);
        set_app_lifecycle_handler(Box::new(|_| {}));
        set_open_urls_handler(Box::new(|_| {}));
    }
    const NOW: Duration = Duration::from_secs(1);
    #[test]
    fn original_touch_pinch_batch_precedes_nested_pause() {
        use std::{cell::RefCell, rc::Rc};
        let state = Rc::new(RefCell::new(ready()));
        let touch = state
            .borrow_mut()
            .touch(1, TouchPhase::Started, position(), NOW)
            .unwrap();
        let pinch = state
            .borrow_mut()
            .pinch(TouchPhase::Started, position(), 1.0, NOW)
            .unwrap();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let output = seen.clone();
        set_mobile_event_handler(Some(Box::new(move |id, event| {
            let name = match event {
                MobileEvent::Touch { event, .. } if event.phase == TouchPhase::Started => "touch",
                MobileEvent::Touch { .. } => "cancel",
                MobileEvent::Pinch {
                    phase: TouchPhase::Started,
                    ..
                } => "pinch",
                MobileEvent::Pinch { .. } => "pinch-cancel",
                MobileEvent::Lifecycle(_) => "pause",
                _ => panic!("unexpected event"),
            };
            output.borrow_mut().push(name);
            if name == "touch" {
                let events = state.borrow_mut().lifecycle(AppLifecycle::Paused, NOW);
                dispatch_batch(id, events);
            }
        })));
        dispatch_batch(1, [touch, pinch]);
        set_mobile_event_handler(None);
        assert_eq!(
            &*seen.borrow(),
            &["touch", "pinch", "cancel", "pinch-cancel", "pause"]
        );
    }
    #[test]
    fn two_pointer_span_reports_scale_and_cancels_on_third_contact() {
        let mut state = ready();
        state.touch(1, TouchPhase::Started, point(px(0.0), px(0.0)), NOW);
        assert!(state.pointer_span(NOW).is_none());
        state.touch(2, TouchPhase::Started, point(px(10.0), px(0.0)), NOW);
        assert!(matches!(
            state.pointer_span(NOW),
            Some(MobileEvent::Pinch {
                phase: TouchPhase::Started,
                scale: 1.0,
                ..
            })
        ));
        state.touch(2, TouchPhase::Moved, point(px(20.0), px(0.0)), NOW);
        assert!(matches!(
            state.pointer_span(NOW),
            Some(MobileEvent::Pinch {
                phase: TouchPhase::Moved,
                scale: 2.0,
                ..
            })
        ));
        state.touch(3, TouchPhase::Started, position(), NOW);
        assert!(matches!(
            state.pointer_span(NOW),
            Some(MobileEvent::Pinch {
                phase: TouchPhase::Cancelled,
                ..
            })
        ));
        assert!(state.pointer_span(NOW).is_none());
    }
    #[test]
    fn terminal_events_with_invalid_coordinates_still_release_state() {
        for phase in [TouchPhase::Ended, TouchPhase::Cancelled] {
            let mut state = ready();
            state
                .touch(1, TouchPhase::Started, position(), NOW)
                .unwrap();
            let terminal = touch(
                state
                    .touch(1, phase, point(px(f32::NAN), px(f32::INFINITY)), NOW)
                    .unwrap(),
            );
            assert_eq!(terminal.position, position());
            assert_eq!(state.active_pointer_count(), 0);
            state
                .pinch(TouchPhase::Started, position(), 1.0, NOW)
                .unwrap();
            assert!(state
                .pinch(phase, point(px(f32::NAN), px(0.0)), f32::NAN, NOW)
                .is_some());
            assert!(state.cancel_all(NOW).is_empty());
        }
    }
    fn position() -> Point<Pixels> {
        point(px(10.0), px(20.0))
    }
    fn ready() -> MobileEventState {
        let mut state = MobileEventState::default();
        state.surface(SurfaceLifecycle::Available, NOW);
        state
    }
    fn touch(event: MobileEvent) -> TouchEvent {
        match event {
            MobileEvent::Touch { event, .. } => event,
            _ => panic!("expected touch"),
        }
    }

    #[test]
    fn cancel_is_not_up_and_native_id_reuse_gets_new_identity() {
        let mut state = ready();
        let down = touch(
            state
                .touch(7, TouchPhase::Started, position(), NOW)
                .unwrap(),
        );
        let cancel = touch(
            state
                .touch(7, TouchPhase::Cancelled, position(), NOW)
                .unwrap(),
        );
        assert_eq!(cancel.phase, TouchPhase::Cancelled);
        assert_eq!(down.id, cancel.id);
        assert!(state.touch(7, TouchPhase::Ended, position(), NOW).is_none());
        let next = touch(
            state
                .touch(7, TouchPhase::Started, position(), NOW)
                .unwrap(),
        );
        assert_ne!(next.id, down.id);
    }

    #[test]
    fn pointers_are_independent_and_cancel_all_drains_once() {
        let mut state = ready();
        let a = touch(
            state
                .touch(1, TouchPhase::Started, position(), NOW)
                .unwrap(),
        );
        let b = touch(
            state
                .touch(2, TouchPhase::Started, position(), NOW)
                .unwrap(),
        );
        assert_ne!(a.id, b.id);
        assert!(state
            .touch(1, TouchPhase::Started, position(), NOW)
            .is_none());
        state.touch(1, TouchPhase::Ended, position(), NOW).unwrap();
        let remaining = state.cancel_all(NOW);
        assert_eq!(remaining.len(), 1);
        assert_eq!(touch(remaining[0].clone()).id, b.id);
        assert!(state.cancel_all(NOW).is_empty());
        assert!(state.touch(2, TouchPhase::Moved, position(), NOW).is_none());
    }

    #[test]
    fn pause_cancels_touch_and_pinch_before_lifecycle() {
        let mut state = ready();
        state
            .touch(1, TouchPhase::Started, position(), NOW)
            .unwrap();
        state
            .pinch(TouchPhase::Started, position(), 1.0, NOW)
            .unwrap();
        let events = state.lifecycle(AppLifecycle::Paused, NOW);
        assert_eq!(events.len(), 3);
        assert_eq!(touch(events[0].clone()).phase, TouchPhase::Cancelled);
        assert!(matches!(
            events[1],
            MobileEvent::Pinch {
                phase: TouchPhase::Cancelled,
                ..
            }
        ));
        assert!(matches!(
            events[2],
            MobileEvent::Lifecycle(AppLifecycle::Paused)
        ));
        assert!(!state.can_render());
        assert!(state
            .touch(1, TouchPhase::Started, position(), NOW)
            .is_none());
        assert!(state.lifecycle(AppLifecycle::Paused, NOW).is_empty());
    }

    #[test]
    fn resume_does_not_restore_lost_surface_or_stale_gesture() {
        let mut state = ready();
        state
            .touch(1, TouchPhase::Started, position(), NOW)
            .unwrap();
        let lost = state.surface(SurfaceLifecycle::Lost, NOW);
        assert_eq!(touch(lost[0].clone()).phase, TouchPhase::Cancelled);
        assert!(matches!(
            lost[1],
            MobileEvent::Surface(SurfaceLifecycle::Lost)
        ));
        state.lifecycle(AppLifecycle::Paused, NOW);
        state.lifecycle(AppLifecycle::Resumed, NOW);
        assert!(!state.can_render());
        state.surface(SurfaceLifecycle::Available, NOW);
        assert!(state.can_render());
        assert!(state.touch(1, TouchPhase::Ended, position(), NOW).is_none());
    }

    #[test]
    fn reject_invalid_coordinates_scale_and_out_of_order_pinch() {
        let mut state = ready();
        assert!(state
            .touch(1, TouchPhase::Started, point(px(f32::NAN), px(0.0)), NOW)
            .is_none());
        assert!(state
            .pinch(TouchPhase::Moved, position(), 2.0, NOW)
            .is_none());
        state
            .pinch(TouchPhase::Started, position(), 1.0, NOW)
            .unwrap();
        assert!(state
            .pinch(TouchPhase::Moved, position(), f32::INFINITY, NOW)
            .is_none());
        assert!(state
            .pinch(TouchPhase::Moved, position(), 0.0, NOW)
            .is_none());
        state
            .pinch(TouchPhase::Moved, position(), 2.0, NOW)
            .unwrap();
        state
            .pinch(TouchPhase::Ended, position(), 2.0, NOW)
            .unwrap();
        assert!(state
            .pinch(TouchPhase::Ended, position(), 2.0, NOW)
            .is_none());
    }
}
