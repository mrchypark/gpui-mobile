use ely_gpui_component::layout::{reveal_in_bounds, reveal_offset};
use gpui::{Bounds, Pixels, ScrollHandle, point, px, size};

/// Choose a vertical scroll destination only for the actual focused field.
fn visible_region(
    focused: bool,
    field: Bounds<Pixels>,
    frame: Bounds<Pixels>,
    keyboard: Option<Bounds<Pixels>>,
) -> Option<Bounds<Pixels>> {
    if !focused {
        return None;
    }
    let keyboard = keyboard?;
    if field.right() <= keyboard.left()
        || field.left() >= keyboard.right()
        || field.bottom() <= keyboard.top()
        || field.top() >= keyboard.bottom()
    {
        return None;
    }
    let above = Bounds::new(
        frame.origin,
        size(
            frame.size.width,
            (keyboard.top().min(frame.bottom()) - frame.top()).max(px(0.)),
        ),
    );
    let below_top = keyboard.bottom().max(frame.top()).min(frame.bottom());
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
    previous: Option<(bool, Bounds<Pixels>, Option<Bounds<Pixels>>)>,
}

impl RevealState {
    fn changed(
        &mut self,
        focused: bool,
        frame: Bounds<Pixels>,
        keyboard: Option<Bounds<Pixels>>,
    ) -> bool {
        let current = (focused, frame, keyboard);
        let changed = self.previous != Some(current);
        self.previous = Some(current);
        changed && focused && keyboard.is_some()
    }

    /// Field position and scroll offset are deliberately not triggers: preserve manual scrolling.
    pub fn reveal(
        &mut self,
        scroll: &ScrollHandle,
        focused: bool,
        field: Bounds<Pixels>,
        keyboard: Option<Bounds<Pixels>>,
    ) -> bool {
        let frame = scroll.bounds();
        self.changed(focused, frame, keyboard)
            && visible_region(focused, field, frame, keyboard)
                .is_some_and(|visible| reveal_in_bounds(scroll, field, visible))
    }
}

#[cfg(any(test, debug_assertions))]
pub fn self_check() {
    let rect = |x, y, w, h| Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
    let frame = rect(0., 0., 300., 180.);
    let field = rect(12., 60., 276., 30.);
    let keyboard = rect(50., 70., 180., 70.);
    assert!(visible_region(false, field, frame, Some(keyboard)).is_none());
    let above = visible_region(true, field, frame, Some(keyboard)).unwrap();
    assert_eq!(above.bottom(), px(70.));
    assert_eq!(reveal_offset(field, above).y, px(-20.));
    let clear = rect(50., 110., 180., 70.);
    assert!(visible_region(true, field, frame, Some(clear)).is_none());
    // Do not pull an offscreen field (200–230) into the keyboard strip (110–180).
    let offscreen = rect(12., 200., 276., 30.);
    assert!(visible_region(true, offscreen, frame, Some(clear)).is_none());
    let side = rect(290., 60., 10., 50.);
    assert!(visible_region(true, field, frame, Some(side)).is_none());
    let moved = rect(50., 0., 180., 80.);
    let below = visible_region(true, field, frame, Some(moved)).unwrap();
    assert_eq!(below.top(), px(80.));
    assert_eq!(reveal_offset(field, below).y, px(20.));
    let resized = rect(50., 65., 180., 75.);
    assert_eq!(
        visible_region(true, field, frame, Some(resized))
            .unwrap()
            .bottom(),
        px(65.)
    );
    assert!(visible_region(true, field, frame, None).is_none());
    let revealed = rect(12., 40., 276., 30.);
    assert!(visible_region(true, revealed, frame, Some(keyboard)).is_none());
    assert!(visible_region(true, field, frame, Some(rect(0., 0., 300., 180.))).is_none());
    let mut state = RevealState::default();
    assert!(state.changed(true, frame, Some(keyboard)));
    assert!(!state.changed(true, frame, Some(keyboard)));
    assert!(!state.changed(false, frame, Some(keyboard)));
    assert!(state.changed(true, frame, Some(keyboard))); // focus acquired again
    assert!(state.changed(true, frame, Some(moved)));
    assert!(state.changed(true, frame, Some(resized)));
    assert!(state.changed(true, rect(0., 0., 300., 160.), Some(resized)));
    assert!(!state.changed(true, frame, None)); // hidden only resets state
    assert!(state.changed(true, frame, Some(keyboard)));
    let scroll = ScrollHandle::new();
    let mut state = RevealState::default();
    state.reveal(&scroll, true, field, Some(keyboard));
    let manual = point(px(0.), px(-55.));
    scroll.set_offset(manual);
    assert!(!state.reveal(&scroll, true, rect(12., -80., 276., 30.), Some(keyboard)));
    assert_eq!(scroll.offset(), manual);
    assert!(!state.reveal(&scroll, true, field, None));
    assert_eq!(scroll.offset(), manual);
}

#[cfg(test)]
mod tests {
    #[test]
    fn focus_and_keyboard_geometry() {
        super::self_check();
    }
}
