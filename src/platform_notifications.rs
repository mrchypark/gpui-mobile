//! Foreground-only notification callback ownership; native bridges only enqueue.
use futures::StreamExt;
use gpui::{ForegroundExecutor, SystemNotificationResponse};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};

#[derive(Default)]
struct Delivery {
    callback: crate::mobile_dispatch::Callback<Box<dyn FnMut(SystemNotificationResponse)>>,
    registered: Cell<bool>,
    draining: Cell<bool>,
    pending: RefCell<VecDeque<SystemNotificationResponse>>,
    started: Cell<bool>,
}
thread_local! { static DELIVERY: Rc<Delivery> = Rc::new(Delivery::default()); }
impl Delivery {
    fn drain(&self) {
        if !self.registered.get() || self.draining.replace(true) {
            return;
        }
        struct Guard<'a>(&'a Cell<bool>);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _guard = Guard(&self.draining);
        loop {
            let response = self.pending.borrow_mut().pop_front();
            let Some(response) = response else {
                break;
            };
            self.callback.call(|| true, |callback| callback(response));
        }
    }
}
pub(super) fn start(executor: ForegroundExecutor) {
    DELIVERY.with(|delivery| {
        if delivery.started.get() {
            return;
        }
        match crate::packages::notifications::take_system_notification_responses() {
            Ok(mut receiver) => {
                delivery.started.set(true);
                let delivery = delivery.clone();
                executor
                    .spawn(async move {
                        while let Some(response) = receiver.next().await {
                            delivery.pending.borrow_mut().push_back(response);
                            delivery.drain();
                        }
                    })
                    .detach();
            }
            Err(error) => log::warn!("Notification response initialization failed: {error}"),
        }
    });
}
pub(super) fn set_handler(
    callback: Box<dyn FnMut(SystemNotificationResponse)>,
    executor: ForegroundExecutor,
) {
    DELIVERY.with(|delivery| {
        delivery.callback.set(Some(callback));
        delivery.registered.set(true);
    });
    // Registration itself can occur while App is borrowed. Deliver only on a
    // later foreground turn, just like responses arriving from the native queue.
    executor
        .spawn(async {
            DELIVERY.with(|delivery| delivery.drain());
        })
        .detach();
}
pub(super) fn poll() {
    if let Err(error) = crate::packages::notifications::poll_system_notification_responses() {
        log::warn!("Notification response poll failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(tag: &str) -> SystemNotificationResponse {
        SystemNotificationResponse {
            tag: tag.into(),
            action_id: None,
        }
    }
    #[test]
    fn callback_can_replace_itself_and_enqueue_without_borrowing_or_losing_order() {
        let delivery = Rc::new(Delivery::default());
        let seen = Rc::new(RefCell::new(Vec::new()));
        delivery
            .pending
            .borrow_mut()
            .push_back(response("before-registration"));
        delivery.drain();
        assert_eq!(delivery.pending.borrow().len(), 1);
        let nested = delivery.clone();
        let values = seen.clone();
        delivery.callback.set(Some(Box::new(move |event| {
            values.borrow_mut().push(event.tag.to_string());
            let next_values = values.clone();
            nested.callback.set(Some(Box::new(move |event| {
                next_values.borrow_mut().push(event.tag.to_string())
            })));
            nested.pending.borrow_mut().push_back(response("nested"));
            nested.drain();
        })));
        delivery.registered.set(true);
        delivery.drain();
        assert_eq!(&*seen.borrow(), &["before-registration", "nested"]);
    }
}
