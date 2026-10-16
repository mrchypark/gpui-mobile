//! Opt-in DEBUG host acceptance; locations arrive only through the native backend.
use super::{demo::{Command, Demo, Snapshot}, LocationSettings};
use std::{sync::atomic::{AtomicBool, Ordering}, time::{Duration, Instant}};
static RUNNING: AtomicBool = AtomicBool::new(false);
struct Running;
impl Drop for Running { fn drop(&mut self) {
    RUNNING.store(false, Ordering::SeqCst);
    let result = crate::android::jni::with_env(|env| {
        let cls = crate::android::jni::find_app_class(env, "dev.gpui.mobile.GpuiServiceProbe")?;
        env.call_static_method(cls, jni::jni_str!("finishLocationWatch"), jni::jni_sig!("()V"), &[])
            .map(|_| ()).map_err(|e| { let _ = env.exception_clear(); e.to_string() })
    });
    if result.is_err() { log::error!("GPS_WATCH_PROBE driver_release_failed=true"); }
} }
#[no_mangle]
pub extern "system" fn Java_dev_gpui_mobile_GpuiServiceProbe_nativeLocationWatch(
    _: *mut std::ffi::c_void, _: *mut std::ffi::c_void,
) -> u8 {
    if RUNNING.swap(true, Ordering::SeqCst) { return 0; }
    if std::thread::Builder::new().name("gps-emulator-watch".into()).spawn(|| {
        let _running = Running;
        log::info!("GPS_WATCH_PROBE begin coordinates_logged=false");
        let result = run();
        log::info!("GPS_WATCH_PROBE end success={} stage={}", result.is_ok(), result.err().unwrap_or("complete"));
    }).is_err() { RUNNING.store(false, Ordering::SeqCst); return 0; }
    1
}
fn move_background(token: u64) -> Result<(), &'static str> {
    use crate::android::jni as helpers;
    let moved = helpers::with_env(|env| {
        let cls = helpers::find_app_class(env, "dev.gpui.mobile.GpuiServiceProbe")?;
        env.call_static_method(cls, jni::jni_str!("moveWatchToBackground"), jni::jni_sig!("(J)Z"),
            &[jni::objects::JValue::Long(token as i64)])
            .and_then(|v| v.z()).map_err(|e| { let _ = env.exception_clear(); e.to_string() })
    }).map_err(|_| "background_move_query")?;
    if moved { Ok(()) } else { Err("background_move_rejected") }
}
fn native(token: u64) -> Result<i32, &'static str> {
    use crate::android::jni as helpers;
    helpers::with_env(|env| {
        let cls = helpers::find_app_class(env, "dev.gpui.mobile.GpuiServiceProbe")?;
        env.call_static_method(cls, jni::jni_str!("locationState"), jni::jni_sig!("(J)I"),
            &[jni::objects::JValue::Long(token as i64)])
            .and_then(|v| v.i()).map_err(|e| { let _ = env.exception_clear(); e.to_string() })
    }).map_err(|_| "native_state_query").and_then(|v| if v < 0 { Err("native_state_unavailable") } else { Ok(v) })
}
fn wait(demo: &Demo, stage: &'static str, predicate: impl Fn(&Snapshot, i32) -> bool) -> Result<Snapshot, &'static str> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let s = demo.snapshot();
        let flags = native(s.native_token)?;
        if predicate(&s, flags) { return Ok(s); }
        if s.finished || Instant::now() >= deadline { return Err(stage); }
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn phase(token: u64, timestamp: u64, boundary: i64, resumed: bool) -> Result<i64, &'static str> {
    use crate::android::jni as helpers;
    helpers::with_env(|env| {
        let cls = helpers::find_app_class(env, "dev.gpui.mobile.GpuiServiceProbe")?;
        env.call_static_method(cls, jni::jni_str!("locationPhase"), jni::jni_sig!("(JJJZ)J"),
            &[jni::objects::JValue::Long(token as i64), jni::objects::JValue::Long(timestamp as i64),
              jni::objects::JValue::Long(boundary), jni::objects::JValue::Bool(resumed)])
            .and_then(|v| v.j()).map_err(|e| { let _ = env.exception_clear(); e.to_string() })
    }).map_err(|_| "native_phase_query").and_then(|v| if v < 0 { Err("native_phase_unavailable") } else { Ok(v) })
}
fn phase_delivery(demo: &Demo, resumed: bool, stage: &'static str) -> Result<(), &'static str> {
    let token = demo.snapshot().native_token;
    let deadline = Instant::now() + Duration::from_secs(60);
    let boundary = loop {
        let boundary = phase(token, 0, 0, resumed)?;
        if boundary > 0 { break boundary; }
        if Instant::now() >= deadline { return Err(stage); }
        std::thread::sleep(Duration::from_millis(50));
    };
    wait(demo, stage, |s, _| s.position.is_some_and(|p|
        phase(token, p.timestamp, boundary, resumed) == Ok(1)))?;
    let gms = native(token)? & 32 != 0;
    let backend = if gms { "gms_fused" } else { "platform" };
    log::info!("GPS_WATCH_PROBE phase={stage} backend={backend} gms_verified={gms} correlated_delivery=true");
    Ok(())
}
fn cleanup(demo: &Demo, background: bool) -> Result<(), &'static str> {
    demo.cancel();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = demo.snapshot();
        if s.finished && native(s.native_token)? & (1 | 16 | if background { 8 } else { 0 }) == 0 {
            std::thread::sleep(Duration::from_millis(250));
            if demo.snapshot().samples != s.samples { return Err("post_cancel_delivery"); }
            log::info!("GPS_WATCH_PROBE stop=true worker_finished=true native_owner_removed=true retired_pending=false background={background} post_cancel_delivery=false");
            return Ok(());
        }
        if Instant::now() >= deadline { return Err("cleanup_pending"); }
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn session(background: bool) -> Result<u64, &'static str> {
    let demo = Demo::spawn().map_err(|_| "demo_spawn")?;
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| "clock")?.as_millis();
    let result = (|| {
        demo.send(Command::Watch { settings: LocationSettings::default(), background }).map_err(|_| "watch_command")?;
        let registered = wait(&demo, "backend_registration", |s, f| s.native_token != 0 && f & 3 == 3
            && ((f & 32 != 0) != (f & 128 != 0)))?;
        let gms = native(registered.native_token)? & 32 != 0;
        let backend = if gms { "gms_fused" } else { "platform" };
        log::info!("GPS_WATCH_PROBE registered=true backend={backend} gms_verified={gms} background={background} awaiting=location_injection");
        let first = wait(&demo, "first_fresh", |s, _| s.samples > 0 && s.position.is_some_and(|p| u128::from(p.timestamp) >= started))?;
        log::info!("GPS_WATCH_PROBE first_fresh=true background={background} samples={}", first.samples);
        if !background {
            let origin = first.position.ok_or("position_missing")?;
            wait(&demo, "movement", |s, _| s.samples > first.samples && s.position.is_some_and(|p|
                super::distance_between(origin.latitude, origin.longitude, p.latitude, p.longitude) > 1.0))?;
            log::info!("GPS_WATCH_PROBE movement=true requesting=app_owned_pause");
            move_background(registered.native_token)?;
            wait(&demo, "foreground_pause", |_, f| f & 4 == 0 && f & 64 != 0)?;
            log::info!("GPS_WATCH_PROBE foreground_paused=true native_listener_removed=true awaiting=host_resume");
            phase_delivery(&demo, true, "foreground_resume")?;
            log::info!("GPS_WATCH_PROBE foreground_resumed=true delivery=true");
        } else {
            log::info!("GPS_WATCH_PROBE background_started=true requesting=app_owned_pause awaiting=location_injection");
            move_background(registered.native_token)?;
            phase_delivery(&demo, false, "background_delivery")?;
            log::info!("GPS_WATCH_PROBE background_delivery=true host_resumed=false fgs_ready=true");
        }
        Ok(registered.native_token)
    })();
    let cleaned = cleanup(&demo, background);
    if let Err(stage) = cleaned { log::error!("GPS_WATCH_PROBE cleanup_failure stage={stage}"); return Err(stage); }
    result
}
fn run() -> Result<(), &'static str> {
    let access = super::request_location_access().map_err(|_| "permission_request")?;
    if !access.can_request_position() { return Err("permission_or_service"); }
    let first = session(false)?;
    log::info!("GPS_WATCH_PROBE restart=true next=background_watch");
    let second = session(true)?;
    if first == second { return Err("restart_token_reused"); }
    Ok(())
}
