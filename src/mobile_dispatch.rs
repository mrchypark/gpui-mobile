//! UI-thread ownership and reentrant callback delivery shared by the mobile backends.
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    rc::{Rc, Weak},
};

pub(crate) struct Registry<T> {
    next: Cell<usize>,
    entries: RefCell<BTreeMap<usize, Weak<T>>>,
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
}

pub(crate) fn set_text_handler(handler: Option<Box<dyn FnMut(&str)>>) {
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

#[cfg(test)]
mod tests {
    use super::*;
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
