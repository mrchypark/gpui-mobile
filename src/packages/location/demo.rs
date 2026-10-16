//! Small standard-example controller. All permission calls run off the UI thread.
use super::{LocationEvent, LocationSettings, LocationSubscription, Position};
use futures::{channel::mpsc, future::{self, AbortHandle, Abortable, Either}, StreamExt};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub enum Command {
    #[cfg(test)]
    PermissionWait(std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>),
    CheckAccess,
    RequestAccess,
    RequestAlways,
    OpenSettings,
    Current(Duration),
    Cache,
    Watch { settings: LocationSettings, background: bool },
    Reconfigure(LocationSettings),
    Refresh,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub status: String,
    pub position: Option<Position>,
    pub cached_preview: Option<Position>,
    pub samples: u64,
    pub last_delivery_unix_ms: u128,
    pub last_live_fix_unix_ms: Option<u64>,
    pub finished: bool,
    pub waiting_since_unix_ms: Option<u128>,
    #[cfg(any(test, all(target_os = "ios", target_env = "sim", debug_assertions), all(target_os = "android", debug_assertions)))]
    pub(super) native_token: u64,
}
impl Snapshot {
    pub fn live_age_text(&self, now: u128) -> String {
        let Some(fix) = self.last_live_fix_unix_ms else { return "No live sample yet".into(); };
        if now < self.last_delivery_unix_ms {
            return "Latest live sample age unavailable (clock moved backward)".into();
        }
        match now.checked_sub(u128::from(fix)) {
            Some(age) => format!("Latest live sample age: {age} ms · {} (5 s UI threshold, not an OS guarantee)",
                if age > 5000 { "Delayed" } else { "Recent" }),
            None => "Latest live sample age unavailable (future fix timestamp / clock mismatch)".into(),
        }
    }
}
pub struct Demo {
    sender: Mutex<mpsc::Sender<Command>>,
    abort: AbortHandle,
    snapshot: Arc<Mutex<Snapshot>>,
}
impl Demo {
    pub fn spawn() -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel(8);
        let (abort, registration) = AbortHandle::new_pair();
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let state = snapshot.clone();
        let worker_abort = abort.clone();
        std::thread::Builder::new().name("gps-demo".into()).spawn(move || {
            let _ = futures::executor::block_on(Abortable::new(run(receiver, state.clone(), worker_abort), registration));
            let mut state = state.lock().unwrap();
            state.finished = true;
            state.waiting_since_unix_ms = None;
            state.status = "Stopped; native listener removal scheduled".into();
        }).map_err(|e| e.to_string())?;
        Ok(Self { sender: Mutex::new(sender), abort, snapshot })
    }
    pub fn send(&self, command: Command) -> Result<(), String> {
        self.sender.lock().unwrap().try_send(command).map_err(|_| "GPS worker busy or closed; Stop cancels pending work".into())
    }
    pub fn snapshot(&self) -> Snapshot { self.snapshot.lock().unwrap().clone() }
    pub fn cancel(&self) { self.abort.abort(); }
}
impl Drop for Demo { fn drop(&mut self) { self.cancel(); } }

// OS authorization may outlive Stop. Keep its independent callback owner alive,
// while allowing Abortable to drop the demo's watch and this result receiver.
async fn permission_wait<T: Send + 'static>(request: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    std::thread::Builder::new().name("gps-permission".into()).spawn(move || {
        if !sender.is_canceled() { let _ = sender.send(request()); }
    }).map_err(|error| error.to_string())?;
    receiver.await.map_err(|_| "Location permission worker disconnected".to_string())?
}

async fn run(mut commands: mpsc::Receiver<Command>, state: Arc<Mutex<Snapshot>>, abort: AbortHandle) {
    let mut watch: Option<LocationSubscription> = None;
    loop {
        // A synchronous permission dialog can outlive Stop. Do not drain queued
        // commands/start new location work when it eventually returns.
        if abort.is_aborted() { return; }
        let incoming = {
        let event = async {
            match watch.as_mut() {
                Some(watch) => watch.next().await,
                None => future::pending().await,
            }
        };
        futures::pin_mut!(event);
        match future::select(commands.next(), event).await {
            Either::Right((event, _)) => Either::Right(event),
            Either::Left((command, _)) => Either::Left(command),
        }
        };
        match incoming {
            Either::Right(event) => {
                if !apply_event(&mut state.lock().unwrap(), event, delivery_time()) { watch = None; }
            }
            Either::Left(command) => {
                let Some(command) = command else { break };
                let result: Result<String, String> = match command {
                    Command::CheckAccess => super::check_location_access().map(|a| format!("{:?}: {}", a.permission, a.guidance())),
                    Command::RequestAccess => permission_wait(super::request_location_access).await.map(|a| format!("{:?}: {}", a.permission, a.guidance())),
                    Command::RequestAlways => permission_wait(|| super::access::request_permission(true)).await.map(|p| format!("Always permission: {p:?}")),
                    #[cfg(test)]
                    Command::PermissionWait(release, entered) => permission_wait(move || {
                        entered.send(()).map_err(|e| e.to_string())?;
                        release.recv().map_err(|e| e.to_string())?;
                        Ok("Permission completed".into())
                    }).await,
                    Command::OpenSettings => crate::packages::permission_handler::open_app_settings().and_then(|opened|
                        if opened { Ok("Settings opened; return and refresh".into()) } else { Err("Settings did not open".into()) }),
                    Command::Current(timeout) => {
                        watch = None;
                        state.lock().unwrap().waiting_since_unix_ms = None;
                        state.lock().unwrap().status = "Waiting for a fresh fix; Stop cancels".into();
                        super::get_current_position_with_timeout(&LocationSettings::default(), timeout).await.map(|position| {
                            let mut state = state.lock().unwrap();
                            state.position = Some(position);
                            state.last_live_fix_unix_ms = Some(position.timestamp);
                            state.samples += 1;
                            state.last_delivery_unix_ms = delivery_time();
                            "Fresh one-shot position received".into()
                        })
                    }
                    Command::Cache => super::get_last_known_position().map(|position| {
                        state.lock().unwrap().position = position;
                        if position.is_some() { "Cached position (may be stale)".into() } else { "No cached position".into() }
                    }),
                    Command::Watch { settings, background } => {
                        state.lock().unwrap().waiting_since_unix_ms = None;
                        // Preview is never a stream delivery or a fresh one-shot result.
                        state.lock().unwrap().cached_preview = super::get_last_known_position().ok().flatten()
                            .filter(|p| preview_eligible(p, delivery_time()));
                        // Stop first to make ownership explicit; a pending Android teardown
                        // returns a retryable startup event rather than silently reusing it.
                        watch = None;
                        let result = if background { super::watch_position_background(&settings) } else { super::watch_position(&settings) };
                        result.map(|subscription| {
                            #[cfg(any(test, all(target_os = "ios", target_env = "sim", debug_assertions), all(target_os = "android", debug_assertions)))]
                            { state.lock().unwrap().native_token = subscription.test_token(); }
                            watch = Some(subscription); "Watch requested; wait for native events".into()
                        })
                    }
                    Command::Reconfigure(settings) => watch.as_mut().ok_or_else(|| "Start a watch first".into())
                        .and_then(|watch| watch.reconfigure(&settings)).map(|_| "Watch settings updated".into()),
                    Command::Refresh => watch.as_ref().ok_or_else(|| "Start a watch first".into())
                        .and_then(LocationSubscription::refresh).map(|_| "Watch refresh requested".into()),
                };
                state.lock().unwrap().status = result.unwrap_or_else(|error| format!("Error: {error}"));
            }
        }
    }
}

fn delivery_time() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default().as_millis()
}

fn preview_eligible(position: &Position, now: u128) -> bool {
    let timestamp = u128::from(position.timestamp);
    timestamp <= now && now - timestamp <= 30_000
        && position.accuracy.is_finite() && (0.0..=100.0).contains(&position.accuracy)
}

fn apply_event(state: &mut Snapshot, event: Option<LocationEvent>, now: u128) -> bool {
    match event {
        Some(LocationEvent::Ready { provider, background }) => {
            state.waiting_since_unix_ms = Some(now);
            state.status = format!("{} tracking registered ({provider}); waiting for {} fix",
                if background { "Background" } else { "Foreground" }, if state.samples == 0 { "first" } else { "next" });
        }
        Some(LocationEvent::Paused) => {
            state.waiting_since_unix_ms = None;
            state.status = "Tracking paused; waiting for foreground resume".into();
        }
        Some(LocationEvent::Position(position)) => {
            state.cached_preview = None;
            state.waiting_since_unix_ms = None;
            state.position = Some(position); state.samples += 1;
            state.last_live_fix_unix_ms = Some(position.timestamp);
            state.last_delivery_unix_ms = now;
            state.status = "Live position received".into();
        }
        Some(LocationEvent::Permission(permission)) => state.status = format!("Location permission: {permission:?}"),
        Some(LocationEvent::Error { code, message }) => {
            state.waiting_since_unix_ms = None; state.status = format!("{code}: {message}");
        }
        Some(LocationEvent::Ended { reason }) => {
            state.waiting_since_unix_ms = None; state.status = format!("Watch ended: {reason}");
        }
        None => {
            state.waiting_since_unix_ms = None;
            if !state.status.starts_with("Watch ended:") { state.status = "Watch closed; start a new watch".into(); }
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_age_threshold_and_clock_mismatch() {
        let mut snapshot = Snapshot::default();
        assert_eq!(snapshot.live_age_text(1000), "No live sample yet");
        let p = super::super::stream::parse_position("1|1|NaN|5|NaN|NaN|NaN|NaN|1000").unwrap();
        apply_event(&mut snapshot, Some(LocationEvent::Position(p)), 1100);
        assert!(snapshot.live_age_text(6000).contains("Recent"));
        assert!(snapshot.live_age_text(6001).contains("Delayed"));
        assert!(snapshot.live_age_text(1099).contains("unavailable"));
        snapshot.last_live_fix_unix_ms = Some(9000);
        assert!(snapshot.live_age_text(7000).contains("unavailable"));
        snapshot.cached_preview = Some(p);
        assert_eq!(snapshot.samples, 1);
        assert_eq!(snapshot.last_live_fix_unix_ms, Some(9000));
    }
    #[test]
    fn stop_retires_watch_while_permission_is_still_pending() {
        use super::super::events;
        let demo = Demo::spawn().unwrap();
        demo.send(Command::Watch { settings: Default::default(), background: true }).unwrap();
        let token = wait_for(&demo, |s| s.native_token != 0).native_token;
        let (release, pending) = std::sync::mpsc::channel();
        let (entered, started) = std::sync::mpsc::channel();
        demo.send(Command::PermissionWait(pending, entered)).unwrap();
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        demo.send(Command::Watch { settings: Default::default(), background: true }).unwrap();
        demo.cancel();
        let stopped = wait_for(&demo, |s| s.finished);
        assert!(!events::send(token, "ready|background|gps".into()));
        assert_eq!(stopped.native_token, token); // queued restart never ran
        release.send(()).unwrap(); // only now allow independent permission owner to finish
    }
    #[test]
    fn bounded_preview_is_not_a_delivery() {
        let mut p = super::super::stream::parse_position("37|127|NaN|5|NaN|NaN|NaN|NaN|1000").unwrap();
        assert!(preview_eligible(&p, 31_000));
        assert!(!preview_eligible(&p, 31_001));
        assert!(!preview_eligible(&p, 999));
        p.accuracy = 101.0;
        assert!(!preview_eligible(&p, 1000));
        p.accuracy = 5.0;
        let mut snapshot = Snapshot { cached_preview: Some(p), ..Default::default() };
        assert_eq!(snapshot.samples, 0);
        apply_event(&mut snapshot, Some(LocationEvent::Position(p)), 1001);
        assert!(snapshot.cached_preview.is_none());
        assert_eq!(snapshot.samples, 1);
    }
    #[test]
    fn registration_wait_fix_error_and_stop_feedback() {
        let mut state = Snapshot::default();
        apply_event(&mut state, Some(LocationEvent::Permission(super::super::LocationPermission::Precise)), 1);
        assert!(state.waiting_since_unix_ms.is_none()); // permission/FGS is not registration
        apply_event(&mut state, Some(LocationEvent::Ready { provider: "gps".into(), background: true }), 10);
        assert_eq!(state.waiting_since_unix_ms, Some(10));
        assert!(state.status.contains("Background tracking registered (gps); waiting for first fix"));
        assert_eq!(state.samples, 0);
        let position = super::super::stream::parse_position("1|1|NaN|5|NaN|NaN|NaN|NaN|20").unwrap();
        apply_event(&mut state, Some(LocationEvent::Position(position)), 21);
        assert_eq!(state.samples, 1);
        assert_eq!(state.last_delivery_unix_ms, 21);
        assert!(state.waiting_since_unix_ms.is_none());
        apply_event(&mut state, Some(LocationEvent::Error { code: "disabled".into(), message: "No enabled provider".into() }), 22);
        assert!(state.status.starts_with("disabled:"));
        assert!(state.waiting_since_unix_ms.is_none());
        apply_event(&mut state, Some(LocationEvent::Ended { reason: "user_stopped".into() }), 23);
        assert!(!apply_event(&mut state, None, 24));
        assert_eq!(state.status, "Watch ended: user_stopped");
    }
    fn wait_for(demo: &Demo, condition: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let snapshot = demo.snapshot();
            if condition(&snapshot) { return snapshot; }
            assert!(std::time::Instant::now() < deadline, "worker did not complete: {}", snapshot.status);
            std::thread::yield_now();
        }
    }
    #[test]
    fn native_mailbox_wakes_running_worker_ready_fix_then_stop() {
        use super::super::events;
        let demo = Demo::spawn().unwrap();
        demo.send(Command::Watch { settings: Default::default(), background: true }).unwrap();
        let started = wait_for(&demo, |s| s.native_token != 0 && events::pending_polls(s.native_token) >= 1);
        let token = started.native_token;
        assert!(started.waiting_since_unix_ms.is_none());
        assert!(events::send(token, "ready|background|gps".into()));
        let ready = wait_for(&demo, |s| s.waiting_since_unix_ms.is_some()
            && events::pending_polls(token) >= 2);
        assert_eq!(ready.samples, 0);
        assert!(ready.status.contains("Background tracking registered (gps); waiting for first fix"));
        assert!(events::send(token, "position|1|1|NaN|5|NaN|NaN|NaN|NaN|1700000000001".into()));
        let received = wait_for(&demo, |s| s.samples == 1 && events::pending_polls(token) >= 3);
        assert_eq!(received.status, "Live position received");
        assert_eq!(received.position.unwrap().timestamp, 1700000000001);
        assert!(received.last_delivery_unix_ms > 0);
        assert!(received.waiting_since_unix_ms.is_none());
        demo.cancel();
        let stopped = wait_for(&demo, |s| s.finished);
        assert_eq!(stopped.samples, 1);
        assert!(stopped.waiting_since_unix_ms.is_none());
        assert!(!events::send(token, "ready|background|gps".into()));
        assert_eq!(events::pending_polls(token), 0); // registry owner removed by Drop
    }
    #[test]
    fn stop_cancels_pending_one_shot_without_waiting_for_deadline() {
        let demo = Demo::spawn().unwrap();
        demo.send(Command::Current(Duration::from_secs(30))).unwrap();
        wait_for(&demo, |s| s.status.starts_with("Waiting for a fresh"));
        demo.cancel();
        let snapshot = wait_for(&demo, |s| s.finished);
        assert_eq!(snapshot.samples, 0);
        assert_eq!(snapshot.last_delivery_unix_ms, 0);
    }
    #[test]
    fn watch_reconfigure_and_explicit_stop() {
        let demo = Demo::spawn().unwrap();
        demo.send(Command::Watch { settings: Default::default(), background: true }).unwrap();
        wait_for(&demo, |s| s.status.starts_with("Watch requested"));
        demo.send(Command::Reconfigure(LocationSettings { distance_filter: 10.0, ..Default::default() })).unwrap();
        wait_for(&demo, |s| s.status == "Watch settings updated");
        assert!(!demo.snapshot().finished);
        demo.cancel();
        wait_for(&demo, |s| s.finished);
    }
}
