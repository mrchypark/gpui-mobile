//! Dedicated BLE screen. Parent wires this module/router; it owns no GPS or IME hooks.
//! Feature-gate this module with `ble`. Navigation and explicit Close differ.
use gpui::{div, prelude::*, rgb, Context};
use gpui_mobile::packages::{ble::{self, demo::{Command, Demo, Feedback}, CharPropFlags}, permission_handler};
use std::{cell::RefCell, time::Duration};
use super::Router;

#[derive(Default)]
struct Screen {
    worker: Option<Demo>, selected: Option<ble::Characteristic>, feedback: Feedback,
    paused: bool,
}
thread_local! { static SCREEN: RefCell<Screen> = RefCell::new(Screen::default()); }

/// Navigation-away hook: preserve an opted-in background connection, stop UI polling.
pub fn dismiss() {
    SCREEN.with(|s| { let mut s = s.borrow_mut(); if let Some(worker) = &s.worker { worker.leave_screen(); }
        s.selected = None; s.feedback.stop_polling(); });
}
/// Explicit user stop / final app-owner teardown, independent of navigation.
pub fn close() {
    SCREEN.with(|s| { let mut s = s.borrow_mut(); if let Some(worker) = &s.worker { worker.cancel(); }
        s.selected = None; });
}
/// Host lifecycle hook. Busy work is cancelled conservatively; an idle opted-in
/// connected session survives pause via the native background context.
pub fn on_pause() {
    SCREEN.with(|s| { let mut s = s.borrow_mut();
        if s.paused { return; } s.paused = true;
        if let Some(worker) = &s.worker {
        // A real permission dialog can pause the host during initial setup.
        // No scan/connection exists yet; do not cancel the user's grant request.
        if worker.snapshot().initializing { return; }
        if worker.send(Command::Pause).is_err() { worker.cancel(); }
    }});
}
pub fn on_resume() {
    let active = SCREEN.with(|s| { let mut s = s.borrow_mut();
        let active = s.paused && s.worker.is_some(); s.paused = false; active });
    if !active { return; }
    let permission = permission_handler::check_permission(permission_handler::Permission::Bluetooth);
    let permission = if permission.is_ok_and(|p| p.is_granted()) { ble::Permission::Granted } else { ble::Permission::Denied };
    SCREEN.with(|s| { if let Some(worker) = &s.borrow().worker { worker.resume(permission); } });
}
fn send(command: Command) {
    SCREEN.with(|s| {
        let mut s = s.borrow_mut();
        s.feedback.message = match s.worker.as_ref().ok_or_else(|| "Open a BLE session first".to_string())
            .and_then(|worker| worker.send(command).map_err(|e| e.to_string())) {
            Ok(()) => String::new(), Err(e) => e,
        };
    });
}
fn open() {
    SCREEN.with(|s| {
        let mut s = s.borrow_mut();
        // Require explicit close before replacing a session; scans are adapter-wide.
        if s.worker.is_some() { s.feedback.message = "Close the existing session first".into(); return; }
        match Demo::spawn(|| {
            let result = permission_handler::request_permission(permission_handler::Permission::Bluetooth)
                .map_err(ble::Error::RuntimeError)?;
            if !result.is_granted() { return Err(ble::Error::PermissionDenied); }
            Ok(ble::Permission::Granted)
        }) {
            Ok(worker) => { s.worker = Some(worker); s.feedback.new_session(); }, Err(e) => s.feedback.message = e.to_string(),
        }
    });
}

pub fn render(_router: &Router, cx: &mut Context<Router>) -> impl IntoElement {
    let epoch = SCREEN.with(|s| s.borrow_mut().feedback.start_polling());
    if let Some(epoch) = epoch {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                if SCREEN.with(|s| !s.borrow().feedback.poll_is_current(epoch)) { break; }
                if this.update(cx, |_, cx| cx.notify()).is_err() { break; }
            }
        }).detach();
    }
    let (snapshot, selected, message, scan_status) = SCREEN.with(|s| {
        let mut s = s.borrow_mut();
        let snapshot = s.worker.as_ref().map(Demo::snapshot).unwrap_or_default();
        if s.worker.is_some() { s.feedback.refresh(&snapshot); }
        (snapshot, s.selected.clone(), s.feedback.message.clone(), s.feedback.scan_status.clone())
    });
    let button = |id: &'static str, label: &'static str| div().id(id).p_3().rounded_md()
        .bg(rgb(0x283950)).text_color(rgb(0xffffff)).child(label);
    let mut content = div().flex().flex_col().gap_3().p_4()
        .child("BLE — select a known device; inspect services before using GATT")
        .child(snapshot.status.clone()).child(message)
        .child(scan_status)
        .child(format!("Worker operations: {} started / {} completed", snapshot.commands_started, snapshot.commands_completed))
        .child(button("ble-open", "Open / request Bluetooth permission").on_click(cx.listener(|_, _, _, cx| { open(); cx.notify(); })))
        .child(button("ble-qcy-scan", "Scan QCY names (10 seconds)").on_click(cx.listener(|_, _, _, cx| {
            send(Command::Scan(ble::ScanOptions { name_prefix: Some("QCY".into()), ..Default::default() })); cx.notify();
        })))
        .child(button("ble-all-scan", "Scan nearby (10 seconds; names/IDs shown locally)").on_click(cx.listener(|_, _, _, cx| {
            send(Command::Scan(ble::ScanOptions::default())); cx.notify();
        })))
        .child(button("ble-disconnect", "Disconnect").on_click(cx.listener(|_, _, _, cx| {
            send(Command::Disconnect); SCREEN.with(|s| s.borrow_mut().selected = None); cx.notify();
        })))
        .child(button("ble-background", "Keep connected in background (requires host setup)").on_click(cx.listener(|_, _, _, cx| {
            send(Command::EnableBackground); cx.notify();
        })))
        .child(button("ble-qcy-read", "Read QCY firmware/battery if advertised readable (no writes)").on_click(cx.listener(|_, _, _, cx| {
            send(Command::ReadQcy); cx.notify();
        })))
        .child(button("ble-close", "Close / cancel and clean up").on_click(cx.listener(|_, _, _, cx| { close(); cx.notify(); })));
    if snapshot.finished {
        SCREEN.with(|s| { let mut s = s.borrow_mut(); s.feedback.message = snapshot.status.clone(); s.worker = None; });
    }
    for (index, device) in snapshot.devices.iter().enumerate() {
        let id = device.id.clone();
        let name = device.properties.local_name.as_deref().or(device.properties.advertisement_name.as_deref()).unwrap_or("Unnamed");
        content = content.child(div().id(("ble-device", index)).p_3().bg(rgb(0x283950)).text_color(rgb(0xffffff))
            .child(format!("Connect + inspect: {name} · {} · RSSI {:?}", device.id, device.properties.rssi))
            .on_click(cx.listener(move |_, _, _, cx| { send(Command::Connect(id.clone())); cx.notify(); })));
    }
    for (index, c) in snapshot.characteristics.iter().enumerate() {
        let c = c.clone();
        content = content.child(div().id(("ble-characteristic", index)).p_2()
            .child(format!("Service {} / {} · {:?}", c.service_uuid, c.uuid, c.properties))
            .on_click(cx.listener(move |_, _, _, cx| {
                SCREEN.with(|s| s.borrow_mut().selected = Some(c.clone())); cx.notify();
            })));
    }
    if let Some(c) = selected.filter(|c| snapshot.characteristics.contains(c)) {
        content = content.child(format!("Selected: {} / {}", c.service_uuid, c.uuid));
        if c.properties.contains(CharPropFlags::READ) {
            let selected = c.clone();
            content = content.child(button("ble-read", "Read selected characteristic").on_click(cx.listener(move |_, _, _, cx| { send(Command::Read(selected.clone())); cx.notify(); })));
        }
        if c.properties.intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE) {
            let selected = c.clone();
            content = content.child(button("ble-notify", "Subscribe selected").on_click(cx.listener(move |_, _, _, cx| { send(Command::Subscribe(selected.clone())); cx.notify(); })));
            let selected = c.clone();
            content = content.child(button("ble-unsubscribe", "Unsubscribe selected").on_click(cx.listener(move |_, _, _, cx| { send(Command::Unsubscribe(selected.clone())); cx.notify(); })));
        }
        content = content.child("No protocol writes in this peer-inspection screen. Use the consenting echo-peer test for write verification.");
    }
    content.child(format!("Notifications: {} · last bytes (local only): {:02x?}", snapshot.notifications, snapshot.value))
        .child(format!("Background enabled: {} · background receipts: {} · last receipt UTC ms: {:?}",
            snapshot.background_enabled, snapshot.background_notifications, snapshot.last_notification_unix_ms))
        .child(format!("QCY read-only result (local only): {:?}", snapshot.qcy))
}
