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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px};
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
