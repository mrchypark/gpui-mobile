//! Explicit debug Simulator launch probe. All samples come from real CoreLocation.
use super::{demo::{Command, Demo, Snapshot}, LocationSettings};
use std::{sync::atomic::{AtomicBool, Ordering}, time::{Duration, Instant}};

extern "C" { fn gpui_location_subscription_active(token: u64) -> bool; }

/// Host calls once active; inert unless SIMCTL_CHILD_GPUI_LOCATION_PROBE=1 was set.
#[no_mangle]
pub extern "C" fn gpui_location_start_simulator_probe() {
    if std::env::var("GPUI_LOCATION_PROBE").as_deref() != Ok("1") { return; }
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) { return; }
    if let Err(error) = std::thread::Builder::new().name("gps-simulator-probe".into()).spawn(|| {
        eprintln!("GPS_SIM_PROBE begin simulated_location=true");
        match run() {
            Ok(()) => eprintln!("GPS_SIM_PROBE end success=true"),
            Err(stage) => eprintln!("GPS_SIM_PROBE end success=false stage={stage}"),
        }
    }) {
        eprintln!("GPS_SIM_PROBE end success=false stage=thread_spawn kind={:?}", error.kind());
    }
}

fn wait(demo: &Demo, seconds: u64, stage: &'static str, predicate: impl Fn(&Snapshot) -> bool) -> Result<Snapshot, &'static str> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let state = demo.snapshot();
        if predicate(&state) { return Ok(state); }
        if state.finished || Instant::now() >= deadline { return Err(stage); }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn run() -> Result<(), &'static str> {
    // Normal OS authorization flow; parent handles the real system prompt.
    let access = super::request_location_access().map_err(|error| {
        let (category, message) = permission_error(&error);
        eprintln!("GPS_SIM_PROBE permission_error category={category} message={message}");
        "permission_request"
    })?;
    if !access.can_request_position() { return Err("permission_or_service"); }
    eprintln!("GPS_SIM_PROBE permission_granted=true");
    let demo = Demo::spawn().map_err(|_| "demo_spawn")?;
    demo.send(Command::Watch { settings: LocationSettings::default(), background: false }).map_err(|_| "watch_command")?;
    let result = (|| {
        let registered = wait(&demo, 10, "registration", |s| s.native_token != 0 && (s.waiting_since_unix_ms.is_some() || s.samples > 0))?;
        eprintln!("GPS_SIM_PROBE registered=true awaiting_simctl_location=true");
        let first = wait(&demo, 60, "first_fix", |s| s.samples > 0 && s.position.is_some())?;
        let origin = first.position.ok_or("first_position")?;
        eprintln!("GPS_SIM_PROBE first_fix=true samples={} timestamp_present={}", first.samples, origin.timestamp > 0);
        let moved = wait(&demo, 60, "moving_updates", |s| s.samples > first.samples && s.position.is_some_and(|p|
            super::distance_between(origin.latitude, origin.longitude, p.latitude, p.longitude) > 1.0))?;
        eprintln!("GPS_SIM_PROBE moving_updates=true samples={} coordinates_logged=false", moved.samples);
        Ok(registered.native_token)
    })();
    let token = demo.snapshot().native_token;
    demo.cancel();
    // Also clean up when a required injected sample never arrives.
    let stopped = wait(&demo, 5, "worker_cancel", |s| s.finished)?;
    if token != 0 {
        if unsafe { gpui_location_subscription_active(token) } { return Err("native_listener_not_removed"); }
        if super::events::send(token, "ready|foreground|late_probe".into()) { return Err("late_event_accepted"); }
        if demo.snapshot().samples != stopped.samples { return Err("post_cancel_delivery"); }
        eprintln!("GPS_SIM_PROBE cancel=true native_listener_removed=true late_mailbox_event_rejected=true");
    }
    result.map(|_| ())
}

fn permission_error(error: &str) -> (&'static str, &'static str) {
    if error.contains("main thread") { ("thread", "permission request requires UIKit main") }
    else if error.contains("while the app is active") { ("lifecycle", "app must be active") }
    else if error.contains("services are disabled") { ("service", "Location Services disabled") }
    else if error.contains("UsageDescription") { ("configuration", "required location usage description missing") }
    else if error.contains("Another permission request") { ("busy", "another permission request is active") }
    else if error.contains("backgrounding") { ("lifecycle", "permission request interrupted by backgrounding") }
    else if error.contains("disconnected") { ("callback", "permission callback disconnected") }
    else { ("native", "permission backend failed; unclassified native detail redacted") }
}
