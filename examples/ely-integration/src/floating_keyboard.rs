use ely_gpui_component::layout::{reveal_in_bounds, reveal_offset};
use gpui::{Bounds, Pixels, ScrollHandle, point, px, size};

fn overlaps(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    a.left() < b.right() && a.right() > b.left() && a.top() < b.bottom() && a.bottom() > b.top()
}

pub fn controls_height(available: Bounds<Pixels>, keyboard: Option<Bounds<Pixels>>) -> Pixels {
    let height = px(180.).min(available.size.height);
    let header = Bounds::new(available.origin, size(available.size.width, height));
    let Some(keyboard) = keyboard.filter(|keyboard| overlaps(header, *keyboard)) else {
        return height;
    };
    // If the floating keyboard covers the header, borrow room from the canvas.
    if keyboard.top() - header.top() < px(64.) && header.bottom() - keyboard.bottom() < px(64.) {
        return (keyboard.bottom() - header.top() + px(64.))
            .max(height)
            .min(available.size.height);
    }
    height
}

/// Choose a vertical scroll destination only for the actual focused field.
fn visible_region(
    field: Bounds<Pixels>,
    frame: Bounds<Pixels>,
    keyboard: Option<Bounds<Pixels>>,
) -> Option<Bounds<Pixels>> {
    let Some(keyboard) = keyboard.filter(|keyboard| overlaps(frame, *keyboard)) else {
        return Some(frame);
    };
    if field.right() <= keyboard.left() || field.left() >= keyboard.right() {
        return Some(frame);
    }
    // Keep the field border clear of the keyboard’s rounded backdrop.
    let gap = px(24.);
    let above = Bounds::new(
        frame.origin,
        size(
            frame.size.width,
            ((keyboard.top() - gap).min(frame.bottom()) - frame.top()).max(px(0.)),
        ),
    );
    let below_top = (keyboard.bottom() + gap)
        .max(frame.top())
        .min(frame.bottom());
    let below = Bounds::new(
        point(frame.left(), below_top),
        size(frame.size.width, frame.bottom() - below_top),
    );
    // shortcut: full control-area occlusion has no scroll destination; reflow the panel to support it.
    [above, below]
        .into_iter()
        .filter(|region| region.size.height >= field.size.height)
        .min_by(|a, b| {
            let a = f32::from(reveal_offset(field, *a).y).abs();
            let b = f32::from(reveal_offset(field, *b).y).abs();
            a.total_cmp(&b)
        })
}

#[derive(Default)]
pub struct RevealState {
    previous: Option<(Option<u8>, Bounds<Pixels>, Option<Bounds<Pixels>>, bool)>,
}

impl RevealState {
    /// Force the next focus acquisition to reveal, including the same field again.
    pub fn focus_lost(&mut self) {
        self.previous = None;
    }

    fn changed(
        &mut self,
        focused: Option<u8>,
        frame: Bounds<Pixels>,
        keyboard: Option<Bounds<Pixels>>,
        keyboard_visible: bool,
    ) -> bool {
        let current = (focused, frame, keyboard, keyboard_visible);
        let changed = self.previous != Some(current);
        self.previous = Some(current);
        changed && focused.is_some() && keyboard_visible
    }

    /// Field position and scroll offset are deliberately not triggers: preserve manual scrolling.
    pub fn reveal(
        &mut self,
        scroll: &ScrollHandle,
        focused: Option<u8>,
        field: Bounds<Pixels>,
        keyboard: Option<Bounds<Pixels>>,
        keyboard_visible: bool,
    ) -> bool {
        let frame = scroll.bounds();
        self.changed(focused, frame, keyboard, keyboard_visible)
            && visible_region(field, frame, keyboard)
                .is_some_and(|visible| reveal_in_bounds(scroll, field, visible))
    }
}

#[cfg(any(test, debug_assertions))]
pub fn self_check() {
    let rect = |x, y, w, h| Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
    let frame = rect(0., 0., 300., 180.);
    let field = rect(12., 60., 276., 30.);
    let keyboard = rect(50., 70., 180., 70.);
    let above = visible_region(field, frame, Some(keyboard)).unwrap();
    assert_eq!(above.bottom(), px(46.));
    assert_eq!(reveal_offset(field, above).y, px(-44.));
    let docked = rect(0., 180., 300., 100.);
    let offscreen = rect(12., 200., 276., 30.);
    assert_eq!(
        reveal_offset(
            offscreen,
            visible_region(offscreen, frame, Some(docked)).unwrap()
        )
        .y,
        px(-50.)
    );
    let moved = rect(50., 0., 180., 80.);
    let below = visible_region(field, frame, Some(moved)).unwrap();
    assert_eq!(below.top(), px(104.));
    assert_eq!(reveal_offset(field, below).y, px(44.));
    let cover = rect(50., 0., 180., 170.);
    let available = rect(0., 0., 300., 400.);
    assert_eq!(controls_height(available, Some(cover)), px(234.));
    assert!(visible_region(field, frame, Some(cover)).is_none());
    let expanded = Bounds::new(
        available.origin,
        size(
            available.size.width,
            controls_height(available, Some(cover)),
        ),
    );
    assert_eq!(
        visible_region(field, expanded, Some(cover)).unwrap().top(),
        px(194.)
    );
    let mut state = RevealState::default();
    assert!(state.changed(Some(0), frame, Some(keyboard), true));
    assert!(!state.changed(Some(0), frame, Some(keyboard), true));
    assert!(state.changed(Some(1), frame, Some(keyboard), true));
    state.focus_lost();
    assert!(state.changed(Some(1), frame, Some(keyboard), true));
    assert!(state.changed(Some(1), frame, Some(moved), true));
    assert!(!state.changed(Some(1), frame, None, false));
    // Android may resize the scroll frame and report IME visibility without a rectangle.
    assert!(state.changed(Some(1), frame, None, true));
    let scroll = ScrollHandle::new();
    let mut state = RevealState::default();
    state.reveal(&scroll, Some(0), field, Some(keyboard), true);
    let manual = point(px(0.), px(-55.));
    scroll.set_offset(manual);
    assert!(!state.reveal(
        &scroll,
        Some(0),
        rect(12., -80., 276., 30.),
        Some(keyboard),
        true
    ));
    assert_eq!(scroll.offset(), manual);
    assert!(!state.reveal(&scroll, Some(0), field, None, false));
    assert_eq!(scroll.offset(), manual);
}

#[cfg(test)]
mod tests {
    #[test]
    fn focus_and_keyboard_geometry() {
        super::self_check();
    }
}
