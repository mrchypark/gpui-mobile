//! Explicit debug host fixture. No automatic scan, connection or GATT operation.
use super::{Ble, CentralEvent, Permission, ScanFilter, ScanOptions, ScanResult};
use crate::packages::{location, permission_handler as permissions};
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
#[path = "probe_diagnostics.rs"]
mod diagnostics;

static BUSY: AtomicBool = AtomicBool::new(false);
static FOREGROUND: AtomicBool = AtomicBool::new(false);
struct Running;
impl Drop for Running { fn drop(&mut self) { BUSY.store(false, Ordering::SeqCst); } }

#[no_mangle]
pub extern "system" fn Java_dev_gpui_mobile_GpuiServiceProbe_nativeForeground(
    _: *mut std::ffi::c_void, _: *mut std::ffi::c_void, resumed: u8,
) { FOREGROUND.store(resumed != 0, Ordering::SeqCst); }

#[no_mangle]
pub extern "system" fn Java_dev_gpui_mobile_GpuiServiceProbe_nativeRun(
    _: *mut std::ffi::c_void, _: *mut std::ffi::c_void, mode: i32,
) {
    if !matches!(mode, 1 | 2 | 3 | 4 | 5 | 6 | 7) || BUSY.swap(true, Ordering::SeqCst) { return; }
    if std::thread::Builder::new().name("gpui-service-probe".into()).spawn(move || {
        let _running = Running;
        log::info!("SERVICE_PROBE begin mode={mode}");
        let result = run(mode);
        log::info!("SERVICE_PROBE end mode={mode} success={}", result.is_ok());
        if let Err(stage) = result { log::warn!("SERVICE_PROBE failure stage={stage}"); }
    }).is_err() { BUSY.store(false, Ordering::SeqCst); }
}

fn run(mode: i32) -> Result<(), &'static str> {
    let permission = if mode == 1 || mode == 4 || mode == 5 || mode == 6 || mode == 7 { permissions::Permission::Bluetooth }
        else { permissions::Permission::LocationWhenInUse };
    let status = permissions::request_permission(permission).map_err(|_| "permission_request")?;
    log::info!("SERVICE_PROBE permission mode={mode} status={status:?}");
    if !status.is_granted() { return Err("permission_not_granted"); }
    // Permission Activity may return before our host's onResume is dispatched.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !FOREGROUND.load(Ordering::SeqCst) {
        if Instant::now() >= deadline { return Err("host_not_resumed"); }
        std::thread::sleep(Duration::from_millis(20));
    }
    if mode == 2 || mode == 3 {
        let started = Instant::now();
        let mut settings = location::LocationSettings::default();
        if mode == 3 { settings.accuracy = location::LocationAccuracy::Medium; }
        let accuracy = if mode == 2 { "high" } else { "medium_network_preferred" };
        log::info!("SERVICE_PROBE gps request_accuracy={accuracy} fresh=true timeout_s=30");
        let position = location::get_current_position(&settings).map_err(|error| {
            log::warn!("SERVICE_PROBE gps request_accuracy={accuracy} error_class={} elapsed_ms={}",
                diagnostics::location_error(&error), started.elapsed().as_millis());
            "fresh_location"
        })?;
        log::info!("SERVICE_PROBE gps request_accuracy={accuracy} fresh=true elapsed_ms={} accuracy_m={} timestamp_present={}",
            started.elapsed().as_millis(), position.accuracy, position.timestamp != 0);
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build()
        .map_err(|_| "runtime")?;
    if mode == 5 || mode == 6 || mode == 7 { runtime.block_on(gatt_echo(mode)) }
    else if mode == 4 { runtime.block_on(beacons()) } else { runtime.block_on(scan()) }
}

async fn gatt_echo(mode: i32) -> Result<(), &'static str> {
    let background = mode == 6 || mode == 7;
    const SERVICE: &str = "928d3811-55b1-4af2-a9e7-1b0c6ac20488";
    const CHARACTERISTIC: &str = "928d3812-55b1-4af2-a9e7-1b0c6ac20488";
    let service = SERVICE.parse().map_err(|_| "service_uuid")?;
    let mut ble = Ble::new(Permission::Granted, Duration::from_secs(15)).await.map_err(|_| "ble_open")?;
    let outcome = async {
        let rows = ble.scan_for(ScanOptions { filter: ScanFilter { services: vec![service] },
            duration: Duration::from_secs(8), ..Default::default() }).await.map_err(|_| "scan")?;
        if rows.len() != 1 { return Err("fixture_not_unique"); }
        let id = rows[0].id.clone();
        log::info!("GATT_ECHO fixture_unique=true");
        ble.connect(&id).await.map_err(|_| "connect")?;
        let services = ble.discover().await.map_err(|_| "discover")?;
        let characteristic = services.iter().flat_map(|s| &s.characteristics)
            .find(|c| c.service_uuid == service && c.uuid.to_string() == CHARACTERISTIC)
            .cloned().ok_or("characteristic")?;
        log::info!("GATT_ECHO connected=true discovered=true");
        let initial = ble.read(&characteristic).await.map_err(|_| "initial_read")?;
        if initial != b"initial" { return Err("initial_value"); }
        let mut values = ble.notifications().await.map_err(|_| "notification_stream")?;
        ble.subscribe(&characteristic).await.map_err(|_| "subscribe")?;
        let payload = b"gpui-echo-fixture";
        ble.write(&characteristic, payload, super::WriteType::WithResponse).await.map_err(|_| "write")?;
        let notification = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(value) = values.next().await {
                if value.service_uuid == service && value.uuid == characteristic.uuid { return Some(value.value); }
            }
            None
        }).await.map_err(|_| "notification_timeout")?.ok_or("notification_closed")?;
        if notification != payload { return Err("notification_value"); }
        if ble.read(&characteristic).await.map_err(|_| "read")? != payload { return Err("read_value"); }
        log::info!("GATT_ECHO read=true write=true notification=true");
        if background {
            let no_response = b"gpui-no-response";
            ble.write(&characteristic, no_response, super::WriteType::WithoutResponse).await.map_err(|_| "write_without_response")?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if ble.read(&characteristic).await.map_err(|_| "read_without_response")? == no_response { break; }
                if Instant::now() >= deadline { return Err("write_without_response_value"); }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            log::info!("GATT_ECHO without_response_readback=true");
            ble.enable_background_connected().await.map_err(|_| "background_enable")?;
            log::info!("GATT_ECHO background_ready=true");
            let deadline = Instant::now() + Duration::from_secs(30);
            while FOREGROUND.load(Ordering::SeqCst) {
                if Instant::now() >= deadline { return Err("background_transition_timeout"); }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            if !ble.pause().await.is_empty() { return Err("background_pause"); }
            ble.check_background().map_err(|_| "background_service")?;
            log::info!("GATT_ECHO background_paused=true service_running=true");
            if mode == 7 {
                let deadline = Instant::now() + Duration::from_secs(1200);
                let mut context_lost = false;
                log::info!("GATT_SURVIVAL ready=true");
                while !FOREGROUND.load(Ordering::SeqCst) && Instant::now() < deadline {
                    if ble.check_background().is_err() && !context_lost {
                        context_lost = true;
                        log::info!("GATT_SURVIVAL service_lost=true");
                    }
                    let next = tokio::time::timeout(Duration::from_secs(1), values.next()).await;
                    if matches!(next, Ok(None)) { return Err("notification_closed"); }
                    if let Ok(Some(value)) = next {
                        if value.service_uuid == service && value.uuid == characteristic.uuid {
                            if let Ok(text) = std::str::from_utf8(&value.value) {
                                if let Some(sequence) = text.strip_prefix("gpui-survival:") {
                                    if sequence.bytes().all(|b| b.is_ascii_digit()) && sequence.len() <= 6 {
                                        log::info!("GATT_SURVIVAL notification={} foreground=false service_running={}", sequence, !context_lost);
                                    }
                                }
                            }
                        }
                    }
                }
                if context_lost { return Err("background_service_lost"); }
                if !FOREGROUND.load(Ordering::SeqCst) { return Err("survival_timeout"); }
                ble.resume(Permission::Granted).map_err(|_| "resume")?;
            } else {
            let received = tokio::time::timeout(Duration::from_secs(30), async {
                while let Some(value) = values.next().await {
                    if value.service_uuid == service && value.uuid == characteristic.uuid && value.value == b"gpui-background-push" { return true; }
                }
                false
            }).await.map_err(|_| "background_notification_timeout")?;
            if !received || FOREGROUND.load(Ordering::SeqCst) { return Err("background_notification_state"); }
            ble.check_background().map_err(|_| "background_service_after_notification")?;
            log::info!("GATT_ECHO background_notification=true foreground=false service_running=true");
            let deadline = Instant::now() + Duration::from_secs(30);
            while !FOREGROUND.load(Ordering::SeqCst) {
                if Instant::now() >= deadline { return Err("foreground_resume_timeout"); }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            ble.resume(Permission::Granted).map_err(|_| "resume")?;
            }
        }
        ble.unsubscribe(&characteristic).await.map_err(|_| "unsubscribe")?;
        ble.disconnect().await.map_err(|_| "disconnect")?;
        let fresh = ble.reconnect(&id).await.map_err(|_| "reconnect")?;
        let characteristic = fresh.iter().flat_map(|s| &s.characteristics)
            .find(|c| c.service_uuid == service && c.uuid.to_string() == CHARACTERISTIC).ok_or("rediscover")?;
        let expected: &[u8] = if background { b"gpui-no-response" } else { payload };
        if ble.read(characteristic).await.map_err(|_| "reconnect_read")? != expected { return Err("reconnect_value"); }
        ble.disconnect().await.map_err(|_| "final_disconnect")?;
        log::info!("GATT_ECHO unsubscribe=true reconnect=true final_read=true disconnect=true");
        Ok(())
    }.await;
    let cleanup = ble.close().await;
    log::info!("GATT_ECHO close_errors={}", cleanup.len());
    if cleanup.is_empty() { outcome } else { Err("close") }
}

// BEGIN standalone beacon checks
mod beacon_checks {
    const PEERS: [(&str, &str); 2] = [
        ("01:ef:be:01:ac:be", "gDevice-bt-beacon-chip-1"),
        ("02:ef:be:01:ac:be", "gDevice-bt-beacon-chip-2"),
    ];
    pub(super) const NAME_PREFIX: &str = "gDevice-bt-beacon-chip-";
    pub(super) fn fixture_identity(id: &str) -> bool {
        PEERS.iter().any(|(expected, _)| id.eq_ignore_ascii_case(expected))
    }
    pub(super) fn valid_rows(rows: &[(String, String)]) -> bool {
        rows.len() == PEERS.len() && PEERS.iter().all(|(expected_id, expected_name)|
            rows.iter().filter(|(id, name)| id.eq_ignore_ascii_case(expected_id)
                && name == expected_name).count() == 1)
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        fn row(id: &str, name: &str) -> (String, String) { (id.into(), name.into()) }
        #[test]
        fn requires_both_actual_names_and_distinct_ids_without_duplicates() {
            let good: Vec<_> = PEERS.iter().map(|(id, name)| row(id, name)).collect();
            assert!(valid_rows(&good));
            assert!(!valid_rows(&[]));
            assert!(!valid_rows(&good[..1]));
            assert!(!valid_rows(&[good[0].clone(), good[0].clone()]));
            assert!(!valid_rows(&[good[0].clone(), row(PEERS[0].0, PEERS[1].1)]));
            assert!(!valid_rows(&[good[0].clone(), row(PEERS[1].0, "QCY")]));
            assert!(!valid_rows(&[row(PEERS[0].0, PEERS[1].1), row(PEERS[1].0, PEERS[0].1)]));
            assert!(!valid_rows(&[good[0].clone(), good[1].clone(), good[0].clone()]));
        }
        #[test]
        fn diagnostics_only_expose_known_synthetic_peer_names() {
            assert!(fixture_identity("01:EF:BE:01:AC:BE"));
            assert!(fixture_identity("02:ef:be:01:ac:be"));
            assert!(!fixture_identity("03:ef:be:01:ac:be"));
            assert!(!fixture_identity("be:ac:01:be:ef:01"));
            assert!(!fixture_identity("gDevice-beacon-1"));
        }
        #[test]
        fn formatter_case_is_normalized_but_names_and_address_shape_are_exact() {
            let upper: Vec<_> = PEERS.iter().map(|(id, name)| row(&id.to_ascii_uppercase(), name)).collect();
            assert!(valid_rows(&upper));
            assert!(PEERS.iter().all(|(_, name)| name.starts_with(NAME_PREFIX)));
            assert!(!fixture_identity("01-EF-BE-01-AC-BE"));
            assert!(!fixture_identity(" 01:ef:be:01:ac:be"));
            assert!(!valid_rows(&[upper[0].clone(), row(PEERS[1].0, &PEERS[1].1.to_ascii_uppercase())]));
        }
    }
}
// END standalone beacon checks

fn check_beacon_rows(rows: &[ScanResult]) -> Result<(), &'static str> {
    let identities: Vec<_> = rows.iter().map(|row| (row.id.to_string(),
        row.properties.local_name.as_ref().or(row.properties.advertisement_name.as_ref())
            .cloned().unwrap_or_default())).collect();
    if beacon_checks::valid_rows(&identities) { Ok(()) } else { Err("beacon_identity_or_dedup") }
}

fn log_beacon_rows(phase: &str, rows: &[ScanResult]) {
    let fixture_count = rows.iter().filter(|row| beacon_checks::fixture_identity(&row.id.to_string())).count();
    log::info!("SERVICE_PROBE beacons diagnostic phase={phase} result_count={} fixture_count={fixture_count} other_count={}",
        rows.len(), rows.len() - fixture_count);
    for row in rows.iter().filter(|row| beacon_checks::fixture_identity(&row.id.to_string())) {
        // Debug formatting escapes controls; lengths bound diagnostic output.
        let local = row.properties.local_name.as_ref().map(|s| s.chars().take(64).collect::<String>());
        let advertised = row.properties.advertisement_name.as_ref().map(|s| s.chars().take(64).collect::<String>());
        log::info!("SERVICE_PROBE beacons fixture_name phase={phase} local={local:?} advertised={advertised:?}");
    }
}

async fn beacons() -> Result<(), &'static str> {
    let mut ble = Ble::new(Permission::Granted, Duration::from_secs(5)).await.map_err(|_| "ble_open")?;
    let outcome = async {
        for phase in ["initial", "restart"] {
            let rows = ble.scan_for(ScanOptions { name_prefix: Some(beacon_checks::NAME_PREFIX.into()),
                duration: Duration::from_secs(5), ..Default::default() }).await.map_err(|_| "beacon_scan")?;
            log_beacon_rows(phase, &rows);
            if let Err(stage) = check_beacon_rows(&rows) {
                // Prefix mismatch otherwise discards the evidence needed to diagnose
                // this synthetic fixture. Diagnostic results never satisfy acceptance.
                match ble.scan_for(ScanOptions { duration: Duration::from_secs(3), ..Default::default() }).await {
                    Ok(diagnostic) => log_beacon_rows("unfiltered_failure", &diagnostic),
                    Err(_) => log::warn!("SERVICE_PROBE beacons diagnostic_unfiltered_failed=true"),
                }
                return Err(stage);
            }
            log::info!("SERVICE_PROBE beacons phase={phase} completed=true matching_count=2 identities_ok=true dedup_ok=true");
            if phase == "restart" { break; }
            let negative = ble.scan_for(ScanOptions { name_prefix: Some("QCY".into()),
                duration: Duration::from_secs(3), ..Default::default() }).await.map_err(|_| "negative_scan")?;
            if !negative.is_empty() { return Err("negative_filter_nonzero"); }
            log::info!("SERVICE_PROBE beacons phase=negative completed=true matching_count=0");
            let cancelled = tokio::time::timeout(Duration::from_secs(2), ble.scan_for(ScanOptions {
                duration: Duration::from_secs(10), ..Default::default()
            })).await;
            if cancelled.is_ok() || !ble.scanning { return Err("scan_not_cancelled_while_active"); }
            ble.stop_scan().await.map_err(|_| "cancel_stop")?;
            log::info!("SERVICE_PROBE beacons phase=cancel cancelled=true stop_ok=true");
        }
        Ok(())
    };
    // A host pause aborts the complete acceptance sequence; cleanup still awaits.
    let foreground_exit = async {
        while FOREGROUND.load(Ordering::SeqCst) { tokio::time::sleep(Duration::from_millis(50)).await; }
    };
    let result = match futures::future::select(Box::pin(outcome), Box::pin(foreground_exit)).await {
        futures::future::Either::Left((result, _)) => result,
        futures::future::Either::Right(_) => Err("beacons_host_paused"),
    };
    let cleanup = ble.close().await;
    log::info!("SERVICE_PROBE beacons close_errors={} gatt_operations=0", cleanup.len());
    if !cleanup.is_empty() { return Err("beacons_close"); }
    result
}

async fn scan() -> Result<(), &'static str> {
    let mut ble = Ble::new(Permission::Granted, Duration::from_secs(5)).await.map_err(|_| "ble_open")?;
    let outcome = async {
        let state = ble.adapter_state().await.map_err(|_| "radio_state")?;
        log::info!("SERVICE_PROBE ble radio={state:?}");
        let mut events = ble.events().await.map_err(|_| "events")?;
        let mut errors = ble.scan_errors();
        ble.start_scan(ScanFilter::default()).await.map_err(|_| "scan_start")?;
        log::info!("SERVICE_PROBE ble scan_started=true duration_s=10");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut discoveries = 0u32;
        while Instant::now() < deadline && FOREGROUND.load(Ordering::SeqCst) {
            let next = futures::future::select(events.next(), errors.next());
            match tokio::time::timeout(Duration::from_millis(200), next).await {
                Ok(futures::future::Either::Left((Some(event), _))) => {
                    if matches!(event, CentralEvent::DeviceDiscovered(_)) { discoveries += 1; }
                    ble.observe(&event).await.map_err(|_| "observe")?;
                }
                Ok(futures::future::Either::Right((Some(code), _))) => {
                    log::warn!("SERVICE_PROBE ble native_scan_error={code}");
                    return Err("scan_callback");
                }
                Ok(_) => return Err("event_stream_closed"),
                Err(_) => {}
            }
        }
        log::info!("SERVICE_PROBE ble discovered_events={discoveries} foreground={}", FOREGROUND.load(Ordering::SeqCst));
        if !FOREGROUND.load(Ordering::SeqCst) { return Err("scan_host_paused"); }
        Ok(())
    }.await;
    // Always attempt both stop_scan and disconnect, including error/foreground exit.
    let cleanup = ble.close().await;
    log::info!("SERVICE_PROBE ble close_errors={} gatt_operations=0", cleanup.len());
    if !cleanup.is_empty() { return Err("ble_close"); }
    outcome
}
