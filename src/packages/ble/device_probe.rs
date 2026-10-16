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
    if !matches!(mode, 1 | 2 | 3 | 4) || BUSY.swap(true, Ordering::SeqCst) { return; }
    if std::thread::Builder::new().name("gpui-service-probe".into()).spawn(move || {
        let _running = Running;
        log::info!("SERVICE_PROBE begin mode={mode}");
        let result = run(mode);
        log::info!("SERVICE_PROBE end mode={mode} success={}", result.is_ok());
        if let Err(stage) = result { log::warn!("SERVICE_PROBE failure stage={stage}"); }
    }).is_err() { BUSY.store(false, Ordering::SeqCst); }
}

fn run(mode: i32) -> Result<(), &'static str> {
    let permission = if mode == 1 || mode == 4 { permissions::Permission::Bluetooth }
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
    if mode == 4 { runtime.block_on(beacons()) } else { runtime.block_on(scan()) }
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
        Ok(())
    }.await;
    // Always attempt both stop_scan and disconnect, including error/foreground exit.
    let cleanup = ble.close().await;
    log::info!("SERVICE_PROBE ble close_errors={} gatt_operations=0", cleanup.len());
    if !cleanup.is_empty() { return Err("ble_close"); }
    outcome
}
