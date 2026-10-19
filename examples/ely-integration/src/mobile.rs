use ely_gpui_component::{
    canvas::{CanvasTouch, TouchAction, TouchContact, Viewport},
    layout::{ViewportInsets, usable_bounds},
};
use gpui::{Bounds, Pixels, TouchPhase, point, px, size};
use gpui_mobile::mobile_events::{AppLifecycle, MobileEvent, SurfaceLifecycle, WindowInsets};
use std::{cell::Cell, collections::{BTreeMap, BTreeSet}, rc::Rc};

pub struct MobileViewport {
    pub view: Viewport,
    pub insets: WindowInsets,
    pub canvas: Bounds<Pixels>,
    contacts: BTreeMap<u64, TouchContact>,
    active: BTreeSet<u64>,
    blocked: bool,
    touch: CanvasTouch,
    drag: Option<((f32, f32), Viewport)>,
    pub commits: usize,
}

impl Default for MobileViewport {
    fn default() -> Self {
        Self {
            view: Viewport::new(0., 0., 1.),
            insets: WindowInsets::default(),
            canvas: Bounds::default(),
            contacts: BTreeMap::new(),
            active: BTreeSet::new(),
            blocked: false,
            touch: CanvasTouch::new(4.),
            drag: None,
            commits: 0,
        }
    }
}

impl MobileViewport {
    pub fn available(&self, window: Bounds<Pixels>) -> Bounds<Pixels> {
        let safe = &self.insets.safe_area;
        // Floating keyboards do not occlude an entire bottom strip.
        let keyboard = self
            .insets
            .keyboard
            .as_ref()
            .filter(|b| {
                b.bottom() >= window.bottom()
                    && b.top() < window.bottom()
                    && b.right() >= window.right() - safe.right
                    && b.left() <= window.left() + safe.left
            })
            .map(|b| (window.bottom() - b.top()).max(px(0.)))
            .unwrap_or(px(0.));
        let ime = self
            .insets
            .keyboard_insets
            .as_ref()
            .map(|e| e.bottom)
            .unwrap_or(px(0.));
        usable_bounds(
            window,
            ViewportInsets {
                top: safe.top,
                right: safe.right,
                bottom: safe.bottom,
                left: safe.left,
            },
            keyboard.max(ime),
        )
    }

    pub fn set_canvas(&mut self, bounds: Bounds<Pixels>) {
        if self.canvas != bounds {
            self.cancel();
            self.canvas = bounds;
        }
    }

    fn action(&mut self, action: TouchAction) {
        match action {
            TouchAction::Begin { at } => self.drag = Some((at, self.view)),
            TouchAction::Drag { at } => {
                if let Some((start, view)) = self.drag {
                    self.view = view.panned((at.0 - start.0, at.1 - start.1));
                }
            }
            TouchAction::End { .. } => {
                self.drag = None;
                self.commits += 1;
            }
            // The second-finger transition cancels only the preview, not the contacts/recognizer.
            TouchAction::Cancel => {
                if let Some((_, view)) = self.drag.take() {
                    self.view = view;
                }
            }
            TouchAction::Viewport(view) => self.view = view,
            TouchAction::None => {}
        }
    }

    fn snapshot(&mut self) {
        let contacts: Vec<_> = self.contacts.values().copied().collect();
        let action = self.touch.update(&contacts, self.view);
        self.action(action);
    }

    fn cancel(&mut self) {
        // Keep physical contacts separate from the cancelled recognizer snapshot.
        self.blocked = !self.active.is_empty();
        let action = self.touch.cancel();
        self.action(action);
        self.contacts.clear();
        self.snapshot();
    }

    pub fn event(&mut self, event: &MobileEvent) {
        match event {
            MobileEvent::Insets(insets) => {
                if self.insets != *insets {
                    self.cancel();
                    self.insets = insets.clone();
                }
            }
            MobileEvent::Lifecycle(AppLifecycle::Paused)
            | MobileEvent::Surface(SurfaceLifecycle::Lost) => {
                self.active.clear();
                self.cancel();
            }
            // CanvasTouch owns recognition; the backend's cumulative pinch stream is ignored.
            MobileEvent::Pinch { .. } => {}
            MobileEvent::Touch { event, .. } => {
                let id = event.id.0;
                match event.phase {
                    TouchPhase::Started => {
                        self.active.insert(id);
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        self.active.remove(&id);
                    }
                    _ => {}
                }
                if self.blocked {
                    self.blocked = !self.active.is_empty();
                    return;
                }
                let p = event.position - self.canvas.origin;
                let contact = TouchContact {
                    id,
                    at: (p.x.into(), p.y.into()),
                };
                match event.phase {
                    // GestureRoute already owns this entire sequence, including extra fingers outside.
                    TouchPhase::Started => {
                        self.contacts.insert(id, contact);
                        self.snapshot();
                    }
                    TouchPhase::Moved if self.contacts.contains_key(&id) => {
                        self.contacts.insert(id, contact);
                        self.snapshot();
                    }
                    TouchPhase::Ended if self.contacts.contains_key(&id) => {
                        self.contacts.insert(id, contact);
                        self.snapshot(); // final location must precede removal
                        self.contacts.remove(&id);
                        self.snapshot();
                    }
                    TouchPhase::Cancelled => {
                        self.cancel();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

// Run against the same adapter and real Ely types before opening the debug simulator UI.
#[cfg(debug_assertions)]
pub fn self_check() {
    use gpui::{Edges, TouchEvent, TouchId};
    use std::time::Duration;
    let event = |id, phase, x, y| MobileEvent::Touch {
        event: TouchEvent {
            id: TouchId(id),
            phase,
            position: point(px(x), px(y)),
            ..Default::default()
        },
        timestamp: Duration::ZERO,
    };
    let mut m = MobileViewport::default();
    m.canvas = Bounds::new(point(px(12.), px(124.)), size(px(300.), px(400.)));
    m.event(&event(1, TouchPhase::Started, 22., 134.));
    assert_eq!(m.contacts[&1].at, (10., 10.));
    m.event(&event(1, TouchPhase::Moved, 42., 134.));
    assert_eq!(m.view.x, -20.);
    m.event(&event(2, TouchPhase::Started, 62., 134.));
    assert_eq!(m.contacts.len(), 2);
    assert_eq!(m.view.x, 0.); // preview cancelled, pinch remains armed
    m.event(&event(2, TouchPhase::Moved, 82., 134.));
    assert_eq!(m.view.zoom, 2.);
    let before = m.view;
    m.event(&MobileEvent::Pinch {
        phase: TouchPhase::Moved,
        center: point(px(0.), px(0.)),
        scale: 3.,
        timestamp: Duration::ZERO,
    });
    assert_eq!(m.view, before); // no double zoom
    m.event(&MobileEvent::Lifecycle(AppLifecycle::Paused));
    m.event(&event(1, TouchPhase::Ended, 42., 134.));
    assert_eq!(m.commits, 0);
    m.event(&event(3, TouchPhase::Started, 22., 134.));
    m.event(&event(3, TouchPhase::Cancelled, 22., 134.));
    m.event(&event(3, TouchPhase::Ended, 22., 134.));
    assert_eq!(m.commits, 0);
    m.event(&event(4, TouchPhase::Started, 22., 134.));
    m.event(&event(4, TouchPhase::Ended, 22., 134.));
    assert_eq!(m.commits, 1);
    m.insets = WindowInsets {
        safe_area: Edges {
            top: px(24.),
            bottom: px(34.),
            left: px(12.),
            right: px(0.),
        },
        keyboard: Some(Bounds::new(
            point(px(0.), px(544.)),
            size(px(390.), px(300.)),
        )),
        ..Default::default()
    };
    let window = Bounds::new(point(px(0.), px(0.)), size(px(390.), px(844.)));
    assert_eq!(m.available(window).size.height, px(520.)); // max(34,300), not their sum
    m.insets.keyboard = Some(Bounds::new(
        point(px(50.), px(300.)),
        size(px(200.), px(200.)),
    ));
    assert_eq!(m.available(window).size.height, px(786.)); // floating keyboard is not a bottom inset
    m.insets.keyboard = Some(Bounds::new(
        point(px(50.), px(544.)),
        size(px(200.), px(300.)),
    ));
    assert_eq!(m.available(window).size.height, px(786.)); // bottom-touching narrow floating keyboard
    let mut route = GestureRoute::new(7);
    route.painted = m.canvas;
    assert!(!route.route(7, &event(10, TouchPhase::Started, 20., 20.)));
    assert!(!route.route(7, &event(11, TouchPhase::Started, 22., 134.))); // extra finger inherits outside
    assert!(!route.route(7, &event(10, TouchPhase::Ended, 22., 134.)));
    assert!(!route.route(7, &event(11, TouchPhase::Ended, 22., 134.)));
    assert!(route.route(7, &event(12, TouchPhase::Started, 22., 134.)));
    assert!(route.route(7, &event(12, TouchPhase::Moved, 20., 20.))); // leaving canvas does not transfer
    assert!(!route.route(8, &event(12, TouchPhase::Started, 22., 134.))); // foreign window is legacy
    assert!(route.route(7, &event(12, TouchPhase::Ended, 20., 20.))); // no final synthetic mouse-up
    assert!(route.route(7, &event(13, TouchPhase::Started, 22., 134.)));
    assert!(route.route(7, &event(13, TouchPhase::Cancelled, 22., 134.)));
    assert!(!route.route(7, &event(14, TouchPhase::Started, 20., 20.))); // next gesture chooses again
    m.event(&event(20, TouchPhase::Started, 22., 134.));
    m.event(&event(21, TouchPhase::Started, 62., 134.));
    m.event(&event(20, TouchPhase::Cancelled, 22., 134.));
    assert!(m.contacts.is_empty());
    m.event(&MobileEvent::Surface(SurfaceLifecycle::Lost));
    assert!(m.drag.is_none());
    let mut outside = MobileViewport::default();
    outside.canvas = m.canvas;
    outside.event(&event(30, TouchPhase::Started, 42., 144.));
    outside.event(&event(30, TouchPhase::Moved, 52., 144.));
    outside.event(&event(31, TouchPhase::Started, 52., 104.)); // header: y below canvas origin
    assert_eq!(outside.contacts.len(), 2);
    assert_eq!(outside.view.x, 0.); // second finger cancels edit preview only
    outside.event(&event(31, TouchPhase::Moved, 52., 64.));
    assert_eq!(outside.view.zoom, 2.);
    assert_eq!(outside.commits, 0);
    let mut final_position = MobileViewport::default();
    final_position.canvas = m.canvas;
    final_position.event(&event(40, TouchPhase::Started, 42., 144.));
    final_position.event(&event(40, TouchPhase::Ended, 62., 154.));
    assert_eq!((final_position.view.x, final_position.view.y), (-20., -10.));
    assert_eq!(final_position.commits, 1);

    let mut cancelled = MobileViewport::default();
    cancelled.canvas = m.canvas;
    let mut route = GestureRoute::new(7);
    route.painted = m.canvas;
    for e in [
        event(50, TouchPhase::Started, 42., 144.),
        event(51, TouchPhase::Started, 62., 144.),
        event(50, TouchPhase::Cancelled, 42., 144.),
        event(52, TouchPhase::Started, 20., 20.), // outside still belongs to raw sequence
        event(51, TouchPhase::Ended, 62., 144.),
        event(52, TouchPhase::Moved, 80., 20.),
    ] {
        assert!(route.route(7, &e));
        cancelled.event(&e);
    }
    assert_eq!(route.active, BTreeSet::from([52]));
    assert_eq!(cancelled.active, BTreeSet::from([52]));
    assert!(cancelled.blocked);
    assert!(cancelled.contacts.is_empty());
    assert!(cancelled.drag.is_none());
    assert_eq!(cancelled.view, Viewport::new(0., 0., 1.));
    assert_eq!(cancelled.commits, 0);
    let release = event(52, TouchPhase::Ended, 80., 20.);
    assert!(route.route(7, &release));
    cancelled.event(&release);
    assert!(!cancelled.blocked);
    assert_eq!(cancelled.commits, 0);
    assert!(!route.route(7, &event(53, TouchPhase::Started, 20., 20.)));
    assert!(!route.route(7, &event(53, TouchPhase::Ended, 20., 20.)));
    for e in [
        event(54, TouchPhase::Started, 42., 144.),
        event(54, TouchPhase::Ended, 62., 154.),
    ] {
        assert!(route.route(7, &e));
        cancelled.event(&e);
    }
    assert_eq!(cancelled.commits, 1);
    assert_eq!((cancelled.view.x, cancelled.view.y), (-20., -10.));

    let mut insets = MobileViewport::default();
    insets.canvas = m.canvas;
    insets.event(&event(60, TouchPhase::Started, 42., 144.));
    insets.event(&event(60, TouchPhase::Moved, 62., 144.));
    for _ in 0..2 {
        insets.event(&MobileEvent::Insets(insets.insets.clone()));
        assert_eq!(insets.view.x, -20.);
        assert!(insets.drag.is_some());
        assert!(insets.contacts.contains_key(&60));
    }
    let mut changed = insets.insets.clone();
    changed.safe_area.top = px(24.);
    insets.event(&MobileEvent::Insets(changed.clone()));
    assert_eq!(insets.insets, changed);
    assert_eq!(insets.view.x, 0.); // changed geometry rolls back the preview
    assert!(insets.blocked);
    assert!(insets.contacts.is_empty());
    insets.event(&event(60, TouchPhase::Ended, 82., 144.));
    assert!(!insets.blocked);
    assert_eq!(insets.commits, 0);
    let mut overlay = GestureRoute::new(7);
    overlay.painted = m.canvas;
    overlay.menu_open.set(true);
    assert!(!overlay.route(7, &event(70, TouchPhase::Started, 22., 134.)));
    // An outside click can close the menu synchronously; its release stays legacy.
    overlay.menu_open.set(false);
    assert!(!overlay.route(7, &event(71, TouchPhase::Started, 42., 144.)));
    assert!(!overlay.route(7, &event(70, TouchPhase::Ended, 22., 134.)));
    assert!(!overlay.route(7, &event(71, TouchPhase::Cancelled, 42., 144.)));
    assert!(!overlay.route(7, &event(71, TouchPhase::Moved, 42., 144.)));
    assert!(overlay.route(7, &event(72, TouchPhase::Started, 22., 134.)));
    // Opening an overlay cannot steal a raw gesture already owned by the canvas.
    overlay.menu_open.set(true);
    assert!(overlay.route(7, &event(73, TouchPhase::Started, 20., 20.)));
    assert!(overlay.route(7, &event(72, TouchPhase::Cancelled, 22., 134.)));
    assert!(overlay.route(7, &event(73, TouchPhase::Ended, 20., 20.)));
    assert!(!overlay.route(7, &event(74, TouchPhase::Started, 22., 134.)));
    assert!(!overlay.route(7, &event(74, TouchPhase::Ended, 22., 134.)));
    for teardown in [
        MobileEvent::Lifecycle(AppLifecycle::Paused),
        MobileEvent::Surface(SurfaceLifecycle::Lost),
    ] {
        overlay.menu_open.set(true);
        overlay.route(7, &event(75, TouchPhase::Started, 22., 134.));
        assert!(!overlay.route(7, &teardown));
        assert!(overlay.menu_open.get());
        assert!(overlay.active.is_empty());
        assert!(!overlay.route(7, &event(76, TouchPhase::Started, 22., 134.)));
        assert!(!overlay.route(7, &event(76, TouchPhase::Ended, 22., 134.)));
        // Only the actual menu close observer changes overlay ownership.
        overlay.menu_open.set(false);
        assert!(overlay.route(7, &event(77, TouchPhase::Started, 22., 134.)));
        assert!(overlay.route(7, &event(77, TouchPhase::Ended, 22., 134.)));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn adapter_sequence_regressions() {
        super::self_check();
    }
}

/// Synchronous observer-only routing: no App access and no nested native pumping.
/// Policy remains latched after release until the next Started chooses a new owner.
pub struct GestureRoute {
    pub owner: u64,
    pub painted: Bounds<Pixels>,
    pub menu_open: Rc<Cell<bool>>,
    active: std::collections::BTreeSet<u64>,
    raw: bool,
}
impl GestureRoute {
    pub fn new(owner: u64) -> Self {
        Self {
            owner,
            painted: Bounds::default(),
            menu_open: Rc::new(Cell::new(false)),
            active: Default::default(),
            raw: false,
        }
    }
    pub fn route(&mut self, window: u64, event: &MobileEvent) -> bool {
        if window != self.owner {
            return false;
        }
        match event {
            MobileEvent::Touch { event, .. } => match event.phase {
                TouchPhase::Started => {
                    if self.active.is_empty() {
                        self.raw = !self.menu_open.get() && self.painted.contains(&event.position);
                    }
                    self.active.insert(event.id.0);
                }
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    self.active.remove(&event.id.0);
                }
                _ => {}
            },
            MobileEvent::Lifecycle(AppLifecycle::Paused)
            | MobileEvent::Surface(SurfaceLifecycle::Lost) => {
                self.active.clear();
                self.raw = false;
            }
            _ => {}
        }
        self.raw
    }
}
