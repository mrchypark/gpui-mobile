//! GPS session persists across navigation until explicit Stop. No coordinate logs/UI.
use gpui::{div, prelude::*, rgb, Context};
use gpui_mobile::packages::location::{self, demo::{Command, Demo}};
use std::{cell::RefCell, time::Duration};
use super::Router;

#[derive(Default)]
struct Screen {
    worker: Option<Demo>,
    message: String,
    polling: bool,
    epoch: u64,
}
thread_local! { static SCREEN: RefCell<Screen> = RefCell::new(Screen::default()); }

/// Navigation stops repainting, not the explicitly owned GPS session.
pub fn dismiss() {
    SCREEN.with(|state| { let mut state = state.borrow_mut(); state.polling = false; state.epoch += 1; });
}
pub fn stop() {
    SCREEN.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(worker) = &state.worker { worker.cancel(); }
        state.message = "Stop requested; waiting for worker cleanup".into();
    });
}
fn send(command: Command) {
    SCREEN.with(|state| {
        let mut state = state.borrow_mut();
        if state.worker.as_ref().is_some_and(|worker| worker.snapshot().finished) { state.worker = None; }
        if state.worker.is_none() {
            match Demo::spawn() {
                Ok(worker) => state.worker = Some(worker),
                Err(error) => { state.message = error; return; }
            }
        }
        state.message = match state.worker.as_ref().unwrap().send(command) {
            Ok(()) => String::new(), Err(error) => error,
        };
    });
}
pub fn render(_router: &Router, cx: &mut Context<Router>) -> impl IntoElement {
    let epoch = SCREEN.with(|state| {
        let mut state = state.borrow_mut();
        if state.polling { None } else { state.polling = true; state.epoch += 1; Some(state.epoch) }
    });
    if let Some(epoch) = epoch {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                if SCREEN.with(|state| state.borrow().epoch != epoch) { break; }
                if this.update(cx, |_, cx| cx.notify()).is_err() { break; }
            }
        }).detach();
    }
    let (snapshot, message) = SCREEN.with(|state| {
        let state = state.borrow();
        (state.worker.as_ref().map(Demo::snapshot).unwrap_or_default(), state.message.clone())
    });
    let button = |id: &'static str, title: &'static str| div().id(id).p_3().rounded_md()
        .bg(rgb(0x283950)).text_color(rgb(0xffffff)).child(title);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    let live_age = snapshot.live_age_text(now);
    div().flex().flex_col().gap_3().p_4()
        .child("GPS — explicit sessions; tracking continues across navigation until Stop")
        .child(snapshot.status).child(message)
        .child(live_age)
        .child(snapshot.waiting_since_unix_ms.map(|since| {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
            format!("Listener registered · waiting {} s for {}fix · no location received yet in this registration", now.saturating_sub(since) / 1000,
                if snapshot.samples == 0 { "first " } else { "next " })
        }).unwrap_or_default())
        .child(format!("Deliveries: {} · last delivery Unix ms: {} · worker finished: {}",
            snapshot.samples, snapshot.last_delivery_unix_ms, snapshot.finished))
        .child(snapshot.position.map(|position| format!("Accuracy: {:.1} m · fix Unix ms: {} (coordinates hidden)",
            position.accuracy, position.timestamp)).unwrap_or_else(|| "No position".into()))
        .child(snapshot.cached_preview.map(|position| {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
            let age = now.checked_sub(u128::from(position.timestamp))
                .map(|age| format!("{age} ms")).unwrap_or_else(|| "unavailable (clock mismatch)".into());
            format!("Cached preview only · age {age} · accuracy {:.1} m · not a fresh delivery", position.accuracy)
        }).unwrap_or_default())
        .child(button("gps-check", "Check permission / service").on_click(cx.listener(|_, _, _, cx| { send(Command::CheckAccess); cx.notify(); })))
        .child(button("gps-permission", "Request location permission").on_click(cx.listener(|_, _, _, cx| { send(Command::RequestAccess); cx.notify(); })))
        .child(button("gps-always", "Request Always permission explicitly (optional)").on_click(cx.listener(|_, _, _, cx| { send(Command::RequestAlways); cx.notify(); })))
        .child(button("gps-settings", "Open app Settings").on_click(cx.listener(|_, _, _, cx| { send(Command::OpenSettings); cx.notify(); })))
        .child(button("gps-current", "Fresh High fix (30 second deadline)").on_click(cx.listener(|_, _, _, cx| { send(Command::Current(Duration::from_secs(30))); cx.notify(); })))
        .child(button("gps-short", "Fresh High fix (5 second deadline)").on_click(cx.listener(|_, _, _, cx| { send(Command::Current(Duration::from_secs(5))); cx.notify(); })))
        .child(button("gps-cache", "Read cache (may be stale)").on_click(cx.listener(|_, _, _, cx| { send(Command::Cache); cx.notify(); })))
        .child(button("gps-watch", "Start foreground watch").on_click(cx.listener(|_, _, _, cx| { send(Command::Watch { settings: Default::default(), background: false }); cx.notify(); })))
        .child(button("gps-background", "Start background watch (ongoing notification / indicator)").on_click(cx.listener(|_, _, _, cx| { send(Command::Watch { settings: Default::default(), background: true }); cx.notify(); })))
        .child(button("gps-reconfigure", "Reconfigure: Medium accuracy, 10 m displacement").on_click(cx.listener(|_, _, _, cx| {
            send(Command::Reconfigure(location::LocationSettings { accuracy: location::LocationAccuracy::Medium, distance_filter: 10.0 })); cx.notify();
        })))
        .child(button("gps-refresh", "Refresh after permission / service change").on_click(cx.listener(|_, _, _, cx| { send(Command::Refresh); cx.notify(); })))
        .child(button("gps-stop", "Stop GPS only / cancel pending fix").on_click(cx.listener(|_, _, _, cx| { stop(); cx.notify(); })))
}
