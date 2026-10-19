//! UI-thread ownership and reentrant callback delivery shared by the mobile backends.
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    rc::{Rc, Weak},
};

/// Shared production ordering for IME characters and control keys. Every user
/// callback may change focus, so revalidate the captured owner between calls.
pub(crate) fn deliver_ime_key(
    input: crate::mobile_events::ImeInput,
    current: impl Fn() -> bool,
    mut legacy: impl FnMut(&str) -> bool,
    mut dispatch: impl FnMut(gpui::PlatformInput) -> gpui::DispatchEventResult,
    mut replace: impl FnMut(&str),
) {
    use crate::mobile_events::ImeInput;
    if !current() {
        return;
    }
    let (key, text, prefer_character_input): (String, String, bool) = match input {
        ImeInput::Commit(text) => {
            // Native IME commits are already text, not printable key events.
            // GPUI key dispatch takes/restores the handler while probing bindings.
            if !legacy(&text) && current() {
                replace(&text);
            }
            return;
        }
        ImeInput::Enter => ("enter".into(), "\n".into(), false),
        ImeInput::Backspace => ("backspace".into(), "\x08".into(), false),
        ImeInput::DeleteBefore { .. } | ImeInput::Compose { .. } | ImeInput::FinishComposition => {
            return
        }
    };
    if prefer_character_input && legacy(&text) {
        return;
    }
    if !current() {
        return;
    }
    let result = dispatch(gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke {
            modifiers: gpui::Modifiers::default(),
            key,
            key_char: prefer_character_input.then(|| text.clone()),
        },
        is_held: false,
        prefer_character_input,
    }));
    if !result.propagate || result.default_prevented || !current() {
        return;
    }
    if !prefer_character_input && legacy(&text) {
        return;
    }
    if current() && text != "\x08" {
        replace(&text);
    }
}

pub(crate) struct Registry<T> {
    next: Cell<usize>,
    entries: RefCell<BTreeMap<usize, Weak<T>>>,
}

/// Keep native-view ownership for the whole Android motion sequence.
#[derive(Default)]
pub(crate) struct NativeTouchRoute {
    native: bool,
}
impl NativeTouchRoute {
    pub fn route(&mut self, down: bool, terminal: bool, hit_test: impl FnOnce() -> bool) -> bool {
        if down {
            self.native = hit_test();
        }
        let native = self.native;
        if terminal {
            self.native = false;
        }
        native
    }
}
impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self {
            next: Cell::new(1),
            entries: RefCell::new(BTreeMap::new()),
        }
    }
}
impl<T> Registry<T> {
    pub fn insert(&self, value: &Rc<T>) -> usize {
        let id = self.next.get();
        self.next.set(
            id.checked_add(1)
                .expect("native window handle space exhausted"),
        );
        self.entries.borrow_mut().insert(id, Rc::downgrade(value));
        id
    }
    pub fn remove(&self, id: usize) {
        self.entries.borrow_mut().remove(&id);
    }
    pub fn get(&self, id: usize) -> Option<Rc<T>> {
        self.entries.borrow().get(&id)?.upgrade()
    }
    pub fn snapshot(&self) -> Vec<Rc<T>> {
        self.entries
            .borrow()
            .values()
            .filter_map(Weak::upgrade)
            .collect()
    }
}

pub(crate) struct Callback<T> {
    value: RefCell<Option<T>>,
    generation: Cell<u64>,
}
impl<T> Default for Callback<T> {
    fn default() -> Self {
        Self {
            value: RefCell::new(None),
            generation: Cell::new(0),
        }
    }
}
impl<T> Callback<T> {
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }
    pub fn take(&self) -> Option<T> {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.value.borrow_mut().take()
    }
    pub fn set(&self, value: Option<T>) {
        self.generation.set(self.generation.get().wrapping_add(1));
        let previous = std::mem::replace(&mut *self.value.borrow_mut(), value);
        drop(previous);
    }
    pub fn call<R>(&self, is_open: impl Fn() -> bool, call: impl FnOnce(&mut T) -> R) -> Option<R> {
        if !is_open() {
            return None;
        }
        let generation = self.generation.get();
        let callback = self.value.borrow_mut().take();
        let mut callback = callback?;
        let result = call(&mut callback);
        if is_open() && self.generation.get() == generation {
            *self.value.borrow_mut() = Some(callback);
        }
        Some(result)
    }
}

type Handler<E> = Box<dyn FnMut(&E)>;
pub(crate) struct EventQueue<E> {
    pending: RefCell<VecDeque<E>>,
    active: Cell<bool>,
    handler: Callback<Handler<E>>,
}
impl<E> Default for EventQueue<E> {
    fn default() -> Self {
        Self {
            pending: RefCell::new(VecDeque::new()),
            active: Cell::new(false),
            handler: Callback::default(),
        }
    }
}
impl<E> EventQueue<E> {
    pub fn set_handler(&self, handler: Option<Handler<E>>) {
        self.handler.set(handler);
    }
    pub fn dispatch(&self, events: impl IntoIterator<Item = E>) {
        self.pending.borrow_mut().extend(events);
        if self.active.replace(true) {
            return;
        }
        struct Guard<'a>(&'a Cell<bool>);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _guard = Guard(&self.active);
        loop {
            let event = self.pending.borrow_mut().pop_front();
            let Some(event) = event else {
                break;
            };
            self.handler.call(|| true, |handler| handler(&event));
        }
    }
}

thread_local! {
    static TEXT: EventQueue<String> = EventQueue::default();
    static TEXT_REGISTERED: Cell<bool> = const { Cell::new(false) };
    static TEXT_GENERATION: Cell<u64> = const { Cell::new(0) };
}

pub(crate) fn set_text_handler(handler: Option<Box<dyn FnMut(&str)>>) {
    TEXT_GENERATION.set(TEXT_GENERATION.get().wrapping_add(1));
    TEXT_REGISTERED.set(handler.is_some());
    TEXT.with(|queue| {
        queue.set_handler(handler.map(|mut handler| {
            Box::new(move |text: &String| handler(text.as_str())) as Handler<String>
        }))
    });
}

pub(crate) fn dispatch_text(text: &str) -> bool {
    if !TEXT_REGISTERED.get() {
        return false;
    }
    TEXT.with(|queue| queue.dispatch([text.to_owned()]));
    true
}

pub(crate) fn has_text_handler() -> bool {
    TEXT_REGISTERED.get()
}

pub(crate) fn text_handler_generation() -> u64 {
    TEXT_GENERATION.get()
}

/// Clamp UIKit's UTF-16 selection to the composing string without splitting a scalar.
pub(crate) fn composing_selection(
    text: &str,
    location: usize,
    length: usize,
) -> std::ops::Range<usize> {
    let mut boundaries = std::iter::once(0).chain(text.chars().scan(0, |offset, character| {
        *offset += character.len_utf16();
        Some(*offset)
    }));
    let end_limit = location.saturating_add(length);
    let mut start = 0;
    let mut end = 0;
    for offset in &mut boundaries {
        if offset <= location {
            start = offset;
        }
        if offset <= end_limit {
            end = offset;
        }
    }
    start..end.max(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ime_legacy_single_delete_is_a_backspace_command_not_a_range() {
        for codepoints in [false, true] {
            let input =
                crate::mobile_events::ime_delete_request(1, codepoints, false, true).unwrap();
            let value = RefCell::new("123".to_owned());
            deliver_ime_key(
                input,
                || true,
                |text| {
                    assert_eq!(text, "\x08");
                    value.borrow_mut().pop();
                    true
                },
                |event| {
                    let gpui::PlatformInput::KeyDown(event) = event else {
                        panic!("expected key")
                    };
                    assert_eq!(event.keystroke.key, "backspace");
                    gpui::DispatchEventResult {
                        propagate: true,
                        default_prevented: false,
                    }
                },
                |_| panic!("legacy backspace must not replace a guessed range"),
            );
            assert_eq!(&*value.borrow(), "12");
            assert!(crate::mobile_events::ime_delete_request(2, codepoints, false, true).is_none());
        }
    }

    #[test]
    fn ime_commit_revalidates_after_legacy_callback_changes_session() {
        let active = Cell::new(true);
        deliver_ime_key(
            crate::mobile_events::ImeInput::Commit("old".into()),
            || active.get(),
            |_| {
                active.set(false);
                false
            },
            |_| panic!("unexpected key dispatch"),
            |_| panic!("stale commit after legacy focus change"),
        );
    }

    #[test]
    fn ime_commit_reaches_replacement_once_without_synthetic_key_dispatch() {
        let received = RefCell::new(Vec::new());
        deliver_ime_key(
            crate::mobile_events::ImeInput::Commit("한🦀".into()),
            || true,
            |_| false,
            |_| panic!("native commit must not enter GPUI printable-key dispatch"),
            |text| received.borrow_mut().push(text.to_owned()),
        );
        assert_eq!(*received.borrow(), vec!["한🦀"]);
    }

    #[test]
    fn ime_enter_binding_precedes_legacy_and_newline_fallback() {
        for prevent in [false, true] {
            let trace = RefCell::new(Vec::new());
            deliver_ime_key(
                crate::mobile_events::ImeInput::Enter,
                || true,
                |_| {
                    trace.borrow_mut().push("legacy");
                    false
                },
                |event| {
                    let gpui::PlatformInput::KeyDown(event) = event else {
                        panic!("expected key")
                    };
                    assert_eq!(event.keystroke.key, "enter");
                    assert!(!event.prefer_character_input);
                    assert_eq!(event.keystroke.key_char, None);
                    trace.borrow_mut().push("binding");
                    gpui::DispatchEventResult {
                        propagate: true,
                        default_prevented: prevent,
                    }
                },
                |text| {
                    assert_eq!(text, "\n");
                    trace.borrow_mut().push("replace");
                },
            );
            assert_eq!(
                *trace.borrow(),
                if prevent {
                    vec!["binding"]
                } else {
                    vec!["binding", "legacy", "replace"]
                }
            );
        }
    }

    #[test]
    fn ime_owner_change_during_dispatch_does_not_insert_into_replacement() {
        let editor = Cell::new(1);
        deliver_ime_key(
            crate::mobile_events::ImeInput::Enter,
            || editor.get() == 1,
            |_| false,
            |_| {
                editor.set(2);
                gpui::DispatchEventResult {
                    propagate: true,
                    default_prevented: false,
                }
            },
            |_| panic!("stale input reached replacement field"),
        );
    }

    #[test]
    fn ime_consumed_legacy_commit_has_no_duplicate_dispatch_or_replacement() {
        deliver_ime_key(
            crate::mobile_events::ImeInput::Commit("x".into()),
            || true,
            |_| true,
            |_| panic!("duplicate dispatch"),
            |_| panic!("duplicate insertion"),
        );
    }

    #[test]
    fn composing_selection_preserves_utf16_scalar_boundaries() {
        assert_eq!(composing_selection("a🦀한", 1, 2), 1..3);
        assert_eq!(composing_selection("a🦀한", 2, 0), 1..1);
        assert_eq!(composing_selection("a🦀한", 3, usize::MAX), 3..4);
        assert_eq!(composing_selection("", usize::MAX, usize::MAX), 0..0);
    }
    #[test]
    fn native_view_ownership_survives_boundary_crossing_and_extra_pointers() {
        let mut route = NativeTouchRoute::default();
        assert!(!route.route(true, false, || false));
        for terminal in [false, false, true] {
            assert!(!route.route(false, terminal, || panic!("only first down hit-tests")));
        }
        assert!(route.route(true, false, || true));
        assert!(route.route(false, false, || false));
        assert!(route.route(false, true, || false));
        assert!(!route.route(true, false, || false));
    }
    #[test]
    fn text_dispatch_can_clear_replace_and_queue_owned_text() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let output = seen.clone();
        set_text_handler(Some(Box::new(move |text| {
            output.borrow_mut().push(text.to_owned());
            assert!(dispatch_text("nested"));
            set_text_handler(None);
            assert!(!dispatch_text("ignored"));
            let output = output.clone();
            set_text_handler(Some(Box::new(move |text| {
                output.borrow_mut().push(text.to_owned());
                set_text_handler(None);
            })));
        })));
        assert!(dispatch_text("first"));
        assert_eq!(&*seen.borrow(), &["first", "nested"]);
        assert!(!dispatch_text("after-clear"));
    }
    struct Window {
        closed: Cell<bool>,
        destroyed: Rc<Cell<usize>>,
    }
    impl Drop for Window {
        fn drop(&mut self) {
            self.destroyed.set(self.destroyed.get() + 1);
        }
    }
    #[test]
    fn dispatch_retains_current_and_closed_next_until_callback_returns() {
        let registry = Registry::default();
        let destroyed = Rc::new(Cell::new(0));
        let a = Rc::new(Window {
            closed: Cell::new(false),
            destroyed: destroyed.clone(),
        });
        let b = Rc::new(Window {
            closed: Cell::new(false),
            destroyed: destroyed.clone(),
        });
        let a_id = registry.insert(&a);
        let b_id = registry.insert(&b);
        let mut owners = vec![a, b];
        let snapshot = registry.snapshot();
        let mut callbacks = 0;
        for window in &snapshot {
            if window.closed.get() {
                continue;
            }
            callbacks += 1;
            owners.iter().for_each(|window| window.closed.set(true));
            registry.remove(a_id);
            registry.remove(b_id);
            owners.clear();
            assert_eq!(destroyed.get(), 0);
            assert!(window.closed.get());
        }
        assert_eq!(callbacks, 1);
        assert!(registry.get(a_id).is_none());
        drop(snapshot);
        assert_eq!(destroyed.get(), 2);
        let new = Rc::new(Window {
            closed: Cell::new(false),
            destroyed,
        });
        assert!(registry.insert(&new) > b_id);
    }
    #[test]
    fn replacement_and_close_prevent_callback_restoration() {
        let slot: Callback<Box<dyn FnMut()>> = Callback::default();
        slot.set(Some(Box::new(|| {})));
        slot.call(|| true, |_| slot.set(None));
        assert!(slot.call(|| true, |_| ()).is_none());
        let closed = Cell::new(false);
        slot.set(Some(Box::new(|| {})));
        slot.call(|| !closed.get(), |_| closed.set(true));
        closed.set(false);
        assert!(slot.call(|| true, |_| ()).is_none());
    }
    #[test]
    fn nested_events_follow_the_entire_original_batch() {
        let queue = Rc::new(EventQueue::default());
        let weak = Rc::downgrade(&queue);
        let observed = Rc::new(RefCell::new(Vec::new()));
        let output = observed.clone();
        queue.set_handler(Some(Box::new(move |event: &u32| {
            output.borrow_mut().push(*event);
            if *event == 1 {
                weak.upgrade().unwrap().dispatch([3]);
            }
        })));
        queue.dispatch([1, 2]);
        assert_eq!(*observed.borrow(), [1, 2, 3]);
    }
    #[test]
    fn nested_handler_replacement_is_used_for_remaining_events() {
        let queue = Rc::new(EventQueue::default());
        let weak = Rc::downgrade(&queue);
        let observed = Rc::new(RefCell::new(Vec::new()));
        let output = observed.clone();
        queue.set_handler(Some(Box::new(move |_: &u32| {
            let output = output.clone();
            weak.upgrade()
                .unwrap()
                .set_handler(Some(Box::new(move |value| {
                    output.borrow_mut().push(*value)
                })));
        })));
        queue.dispatch([1, 2, 3]);
        assert_eq!(*observed.borrow(), [2, 3]);
    }
}
