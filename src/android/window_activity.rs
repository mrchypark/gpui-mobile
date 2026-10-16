//! Surface availability is independent of OS focus and its delivered callback.
use std::{collections::VecDeque, sync::Mutex};

type Callback = Box<dyn FnMut(bool) + Send>;
struct State {
    active: bool,
    surface: bool,
    callback: Option<Callback>,
    generation: u64,
    dispatching: bool,
    pending: VecDeque<bool>,
}

pub(super) struct WindowActivity(Mutex<State>);

impl WindowActivity {
    pub fn new(active: bool, surface: bool) -> Self {
        Self(Mutex::new(State {
            active,
            surface,
            callback: None,
            generation: 0,
            dispatching: false,
            pending: VecDeque::new(),
        }))
    }
    pub fn is_active(&self) -> bool {
        self.0.lock().unwrap().active
    }
    pub fn has_surface(&self) -> bool {
        self.0.lock().unwrap().surface
    }
    pub fn set_surface(&self, available: bool) {
        let mut state = self.0.lock().unwrap();
        state.surface = available;
    }
    pub fn on_change(&self, callback: Callback) {
        let previous = {
            let mut state = self.0.lock().unwrap();
            state.generation += 1;
            state.callback.replace(callback)
        };
        drop(previous);
    }
    pub fn set_active(&self, active: bool) {
        let mut state = self.0.lock().unwrap();
        if state.active != active {
            state.active = active;
            state.pending.push_back(active);
        }
        drop(state);
        self.flush();
    }
    pub fn flush(&self) {
        let mut state = self.0.lock().unwrap();
        if state.dispatching {
            return;
        }
        state.dispatching = true;
        while let Some(active) = state.pending.pop_front() {
            let generation = state.generation;
            let Some(mut callback) = state.callback.take() else {
                state.pending.push_front(active);
                break;
            };
            drop(state);
            callback(active);
            state = self.0.lock().unwrap();
            if state.generation == generation {
                state.callback = Some(callback);
            } else {
                drop(state);
                drop(callback);
                state = self.0.lock().unwrap();
            }
        }
        state.dispatching = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn recreation_does_not_consume_gained_focus() {
        let activity = WindowActivity::new(true, true);
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let result = delivered.clone();
        activity.on_change(Box::new(move |active| result.lock().unwrap().push(active)));
        activity.set_active(false);
        activity.set_surface(false);
        activity.set_surface(true);
        activity.set_active(true);
        activity.set_active(true);
        assert_eq!(*delivered.lock().unwrap(), [false, true]);
    }

    #[test]
    fn surface_does_not_claim_focus_and_pending_focus_survives_registration() {
        let activity = WindowActivity::new(false, false);
        activity.set_surface(true);
        assert!(!activity.is_active());
        assert!(activity.has_surface());
        activity.set_active(true);
        activity.set_active(false);
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let result = delivered.clone();
        activity.on_change(Box::new(move |active| result.lock().unwrap().push(active)));
        activity.flush();
        assert_eq!(*delivered.lock().unwrap(), [true, false]);
        assert!(!activity.is_active());
    }

    #[test]
    fn reentrant_focus_and_replacement_are_not_lost() {
        let activity = Arc::new(WindowActivity::new(false, false));
        let weak = Arc::downgrade(&activity);
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let result = delivered.clone();
        activity.on_change(Box::new(move |active| {
            result.lock().unwrap().push(active);
            let activity = weak.upgrade().unwrap();
            let result = result.clone();
            activity.on_change(Box::new(move |active| result.lock().unwrap().push(active)));
            activity.set_active(false);
        }));
        activity.set_active(true);
        activity.set_active(true);
        assert_eq!(*delivered.lock().unwrap(), [true, false, true]);
    }
}
