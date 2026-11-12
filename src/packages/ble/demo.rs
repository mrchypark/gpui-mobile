//! Worker for an explicitly opened BLE demo. No scans/connections/writes on startup.
use super::*;
use futures::StreamExt;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};

#[derive(Clone, Default)]
pub struct Snapshot {
    pub finished: bool,
    pub initializing: bool,
    pub background_enabled: bool,
    pub background_notifications: u64,
    pub last_notification_unix_ms: Option<u128>,
    pub qcy: Option<super::qcy::Readout>,
    pub status: String,
    /// Last scan outcome survives subsequent lifecycle status updates.
    pub scan_status: String,
    pub commands_started: u64,
    pub commands_completed: u64,
    pub devices: Vec<ScanResult>,
    pub characteristics: Vec<Characteristic>,
    pub value: Vec<u8>,
    pub notifications: u64,
}
impl Snapshot {
    fn scan_started(&mut self, duration: Duration) {
        self.devices.clear();
        self.scan_status = format!("Scanning for up to {} seconds…", duration.as_secs_f32());
    }
    fn scan_completed(&mut self, devices: Vec<ScanResult>) {
        self.scan_status = format!(
            "Scan completed: {} matching devices.{}",
            devices.len(),
            if devices.is_empty() {
                " No matching advertisements received; check that your peer is advertising, then retry."
            } else {
                ""
            }
        );
        self.devices = devices;
    }
    fn scan_cancelled(&mut self, reason: &str) {
        self.scan_status = format!(
            "Scan cancelled: {reason}. Results are incomplete; reopen the session and retry."
        );
    }
}
/// Screen feedback retained across refreshes and navigation, without owning BLE.
#[derive(Default)]
pub struct Feedback {
    pub message: String,
    pub scan_status: String,
    completed: u64,
    polling: bool,
    epoch: u64,
}
impl Feedback {
    pub fn refresh(&mut self, snapshot: &Snapshot) {
        if snapshot.commands_completed != self.completed || snapshot.finished {
            self.message.clear();
        }
        self.completed = snapshot.commands_completed;
        if !snapshot.scan_status.is_empty() {
            self.scan_status = snapshot.scan_status.clone();
        }
        if snapshot.finished {
            self.message = snapshot.status.clone();
        }
    }
    pub fn start_polling(&mut self) -> Option<u64> {
        if self.polling {
            return None;
        }
        self.polling = true;
        self.epoch += 1;
        Some(self.epoch)
    }
    pub fn stop_polling(&mut self) {
        self.polling = false;
        self.epoch += 1;
    }
    pub fn poll_is_current(&self, epoch: u64) -> bool {
        self.polling && self.epoch == epoch
    }
    pub fn new_session(&mut self) {
        self.message.clear();
        self.scan_status.clear();
        self.completed = 0;
    }
}
pub enum Command {
    EnableBackground,
    Pause,
    Resume(Permission),
    ReadQcy,
    Scan(ScanOptions),
    Connect(PeripheralId),
    Disconnect,
    Read(Characteristic),
    Write(Characteristic, Vec<u8>, WriteType),
    Subscribe(Characteristic),
    Unsubscribe(Characteristic),
}
/// Drop requests cancellation without blocking UI. The worker awaits native
/// stop/disconnect before shutting down its runtime. Inspect snapshot for errors.
pub struct Demo {
    sender: mpsc::SyncSender<Command>,
    snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    lifecycle: Arc<Mutex<VecDeque<Command>>>,
}
impl Demo {
    /// Permission callback runs on the worker and must use the real OS result.
    /// The callback's own permission-dialog lifetime remains the host's contract.
    pub fn spawn(permission: impl FnOnce() -> Result<Permission> + Send + 'static) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let snapshot = Arc::new(Mutex::new(Snapshot {
            initializing: true,
            status: "Checking permission".into(),
            ..Default::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(true));
        let lifecycle = Arc::new(Mutex::new(VecDeque::new()));
        let transitions = lifecycle.clone();
        let (state, cancelled, working) = (snapshot.clone(), stop.clone(), busy.clone());
        std::thread::Builder::new()
            .name("ble-demo".into())
            .spawn(move || {
                let result = (|| {
                    let permission = permission()?;
                    if cancelled.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_time()
                        .build()
                        .map_err(|error| Error::RuntimeError(error.to_string()))?;
                    runtime.block_on(run(
                        receiver,
                        &state,
                        &cancelled,
                        &working,
                        &transitions,
                        permission,
                    ))
                })();
                if let Err(error) = result {
                    state.lock().unwrap().status = error.to_string();
                }
                state.lock().unwrap().finished = true;
                state.lock().unwrap().initializing = false;
                state.lock().unwrap().background_enabled = false;
                working.store(false, Ordering::SeqCst);
            })
            .map_err(|error| Error::RuntimeError(error.to_string()))?;
        Ok(Self {
            sender,
            snapshot,
            stop,
            busy,
            lifecycle,
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = self.snapshot.lock().unwrap().clone();
        if self.busy.load(Ordering::SeqCst) && snapshot.status == "Ready" {
            snapshot.status = "Request queued; waiting for BLE worker".into();
        }
        snapshot
    }
    pub fn send(&self, command: Command) -> Result<()> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(Error::NotSupported("demo is closing".into()));
        }
        if matches!(command, Command::Pause | Command::Resume(_)) {
            let mut lifecycle = self.lifecycle.lock().unwrap();
            // Preserve transition and permission ordering even during native cleanup.
            // Overflow closes conservatively instead of dropping a pause/revocation.
            if lifecycle.len() == 8 {
                self.cancel();
                return Err(Error::NotSupported(
                    "BLE lifecycle queue overflow; closing".into(),
                ));
            }
            lifecycle.push_back(command);
            return Ok(());
        }
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(Error::NotSupported(
                "wait for the active BLE operation".into(),
            ));
        }
        if self.sender.try_send(command).is_err() {
            self.busy.store(false, Ordering::SeqCst);
            return Err(Error::NotSupported("BLE worker is unavailable".into()));
        }
        eprintln!("BLE_DEMO command_queued");
        Ok(())
    }
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
    /// Navigation away preserves explicit background sessions. It is not Close.
    pub fn leave_screen(&self) {
        if !self.snapshot().background_enabled {
            self.cancel();
        }
    }
    /// Lifecycle refresh must not be lost because a preceding pause is finishing.
    /// Pause and resume share a bounded FIFO, including permission revocations.
    pub fn resume(&self, permission: Permission) {
        let _ = self.send(Command::Resume(permission));
    }
}
impl Drop for Demo {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn should_cancel(stop: &AtomicBool, interrupt_on_pause: Option<&Mutex<VecDeque<Command>>>) -> bool {
    stop.load(Ordering::SeqCst)
        || interrupt_on_pause.is_some_and(|queue| {
            queue
                .lock()
                .unwrap()
                .iter()
                .any(|command| matches!(command, Command::Pause))
        })
}
async fn cancellation(stop: &AtomicBool, interrupt_on_pause: Option<&Mutex<VecDeque<Command>>>) {
    while !should_cancel(stop, interrupt_on_pause) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
fn next_command(
    receiver: &mpsc::Receiver<Command>,
    lifecycle: &Mutex<VecDeque<Command>>,
) -> std::result::Result<Command, mpsc::TryRecvError> {
    lifecycle
        .lock()
        .unwrap()
        .pop_front()
        .map(Ok)
        .unwrap_or_else(|| receiver.try_recv())
}
// The production scheduler is also exercised with a deterministic native-operation
// boundary in tests. Neither command admission nor executor polling is replaced.
async fn drive(
    receiver: &mpsc::Receiver<Command>,
    lifecycle: &Mutex<VecDeque<Command>>,
    stop: &AtomicBool,
    mut step: impl AsyncFnMut(std::result::Result<Command, mpsc::TryRecvError>) -> Result<bool>,
) -> Result<()> {
    while !stop.load(Ordering::SeqCst) {
        if !step(next_command(receiver, lifecycle)).await? {
            break;
        }
    }
    Ok(())
}
async fn execute(
    state: &Mutex<Snapshot>,
    busy: &AtomicBool,
    stop: &AtomicBool,
    interrupt_on_pause: Option<&Mutex<VecDeque<Command>>>,
    is_scan: bool,
    operation: impl Future<Output = Result<()>>,
) -> Option<&'static str> {
    busy.store(true, Ordering::SeqCst);
    {
        let mut snapshot = state.lock().unwrap();
        snapshot.status = "Working".into();
        snapshot.commands_started += 1;
        eprintln!(
            "BLE_DEMO command_started count={}",
            snapshot.commands_started
        );
    }
    match futures::future::select(
        Box::pin(operation),
        Box::pin(cancellation(stop, interrupt_on_pause)),
    )
    .await
    {
        futures::future::Either::Left((result, _)) => {
            let mut snapshot = state.lock().unwrap();
            if is_scan {
                if let Err(error) = &result {
                    snapshot.scan_status = format!("Scan failed: {error}");
                    eprintln!("BLE_DEMO scan_failed");
                }
            }
            snapshot.status = match result {
                Ok(()) => "Ready".into(),
                Err(e) => e.to_string(),
            };
            snapshot.commands_completed += 1;
            eprintln!(
                "BLE_DEMO command_completed count={}",
                snapshot.commands_completed
            );
            busy.store(false, Ordering::SeqCst);
            None
        }
        futures::future::Either::Right(_) => {
            let reason = if stop.load(Ordering::SeqCst) {
                "session closed or cancellation requested"
            } else {
                "app paused during BLE operation"
            };
            if is_scan {
                state.lock().unwrap().scan_cancelled(reason);
                eprintln!("BLE_DEMO scan_cancelled reason={reason}");
            }
            Some(if stop.load(Ordering::SeqCst) {
                "Closed after cancellation; scan and connection stopped"
            } else {
                "Closed because app paused during BLE operation; reopen to retry"
            })
        }
    }
}
async fn run(
    receiver: mpsc::Receiver<Command>,
    state: &Mutex<Snapshot>,
    stop: &AtomicBool,
    busy: &AtomicBool,
    lifecycle: &Mutex<VecDeque<Command>>,
    permission: Permission,
) -> Result<()> {
    let mut ble = Ble::new(permission, Duration::from_secs(5)).await?;
    let mut close_reason = "Closed; scan and connection stopped";
    let result: Result<()> = async {
        let mut events = ble.events().await?;
        let mut values: BoxStream<'static, ValueNotification> = Box::pin(futures::stream::pending());
        let mut subscribed = None;
        let radio = ble.adapter_state().await?;
        state.lock().unwrap().status = format!("Radio: {radio:?}");
        state.lock().unwrap().initializing = false;
        let started = std::time::Instant::now();
        busy.store(false, Ordering::SeqCst);
        drive(&receiver, lifecycle, stop, async |command| {
            state.lock().unwrap().background_enabled = ble.background.is_some();
            match command {
                Ok(command) => {
                    let is_scan = matches!(command, Command::Scan(_));
                    // Pause interrupts ordinary work, but must not cancel an earlier
                    // lifecycle transition's cleanup or permission revocation.
                    let interrupt_on_pause = if matches!(command, Command::Pause | Command::Resume(_)) {
                        None
                    } else { Some(lifecycle) };
                    let operation = async {
                        match command {
                            Command::ReadQcy => {
                                let readout = super::qcy::read_known_values(&mut ble).await?;
                                state.lock().unwrap().qcy = Some(readout);
                            }
                            Command::EnableBackground => {
                                ble.enable_background_connected().await?;
                                state.lock().unwrap().background_enabled = true;
                            }
                            Command::Pause => {
                                let errors = ble.pause().await;
                                if ble.state.paused {
                                    state.lock().unwrap().background_enabled = false;
                                    state.lock().unwrap().characteristics.clear();
                                    subscribed = None;
                                    values = Box::pin(futures::stream::pending());
                                }
                                if !errors.is_empty() { return Err(Error::RuntimeError(format!("pause cleanup: {errors:?}"))); }
                            }
                            Command::Resume(permission) => {
                                let errors = ble.permission_changed(permission).await;
                                if !errors.is_empty() { return Err(Error::RuntimeError(format!("permission cleanup: {errors:?}"))); }
                                ble.resume(permission)?;
                            }
                            Command::Scan(options) => {
                                state.lock().unwrap().scan_started(options.duration);
                                eprintln!("BLE_DEMO scan_started duration_ms={}", options.duration.as_millis());
                                let devices = ble.scan_for(options).await?;
                                eprintln!("BLE_DEMO scan_completed matching_count={}", devices.len());
                                state.lock().unwrap().scan_completed(devices);
                            }
                            Command::Connect(id) => {
                                state.lock().unwrap().characteristics.clear();
                                state.lock().unwrap().qcy = None;
                                state.lock().unwrap().value.clear();
                                let services = ble.connect_and_discover(&id).await?;
                                state.lock().unwrap().characteristics = services.into_iter().flat_map(|s| s.characteristics).collect();
                            }
                            Command::Disconnect => {
                                values = Box::pin(futures::stream::pending());
                                subscribed = None;
                                state.lock().unwrap().characteristics.clear();
                                ble.disconnect().await?;
                                state.lock().unwrap().background_enabled = false;
                            }
                            Command::Read(c) => {
                                let value = ble.read(&c).await?;
                                state.lock().unwrap().value = value;
                            }
                            Command::Write(c, data, kind) => { ble.write(&c, &data, kind).await?; }
                            Command::Subscribe(c) => {
                                if subscribed.is_some() { return Err(Error::NotSupported("Unsubscribe the active characteristic first".into())); }
                                values = ble.subscribe_values(&c).await?;
                                subscribed = Some(c);
                            }
                            Command::Unsubscribe(c) => {
                                if subscribed.as_ref() != Some(&c) { return Err(Error::NotSupported("Select the active subscription".into())); }
                                ble.unsubscribe(&c).await?;
                                values = Box::pin(futures::stream::pending());
                                subscribed = None;
                            }
                        }
                        Ok::<_, Error>(())
                    };
                    if let Some(reason) = execute(state, busy, stop, interrupt_on_pause, is_scan, operation).await {
                        close_reason = reason;
                        return Ok(false);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => return Ok(false),
                Err(mpsc::TryRecvError::Empty) => {
                    ble.check_background()?;
                    let next = tokio::time::timeout(Duration::from_millis(25),
                        futures::future::select(events.next(), values.next())).await;
                    match next {
                        Ok(futures::future::Either::Left((Some(event), _))) => {
                            ble.observe(&event).await?;
                            if ble.state.recovery_needed {
                                state.lock().unwrap().characteristics.clear();
                                state.lock().unwrap().status = "Connection changed; disconnect before retry".into();
                                values = Box::pin(futures::stream::pending());
                                subscribed = None;
                                ble.disconnect().await?;
                                state.lock().unwrap().background_enabled = false;
                            }
                        }
                        Ok(futures::future::Either::Right((Some(value), _))) => {
                            let mut snapshot = state.lock().unwrap();
                            snapshot.value = value.value; snapshot.notifications += 1;
                            if ble.in_background { snapshot.background_notifications += 1; }
                            snapshot.last_notification_unix_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis());
                            let receipt = (snapshot.notifications, snapshot.background_notifications, snapshot.last_notification_unix_ms);
                            drop(snapshot);
                            eprintln!("BLE_DEMO notification count={} background={} background_count={} utc_ms={:?} elapsed_ms={}",
                                receipt.0, ble.in_background, receipt.1, receipt.2, started.elapsed().as_millis());
                        }
                        Ok(_) => return Err(Error::NotConnected),
                        Err(_) => {}
                    }
                }
            }
            Ok(true)
        }).await
    }.await;
    let errors = ble.close().await;
    state.lock().unwrap().characteristics.clear();
    if !errors.is_empty() {
        return Err(Error::RuntimeError(format!(
            "BLE cleanup failed: {errors:?}"
        )));
    }
    result?;
    state.lock().unwrap().status = close_reason.into();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn controller() -> (Demo, mpsc::Receiver<Command>) {
        let (sender, receiver) = mpsc::sync_channel(1);
        (
            Demo {
                sender,
                snapshot: Arc::new(Mutex::new(Snapshot::default())),
                stop: Arc::new(AtomicBool::new(false)),
                busy: Arc::new(AtomicBool::new(false)),
                lifecycle: Arc::new(Mutex::new(VecDeque::new())),
            },
            receiver,
        )
    }
    async fn until(mut predicate: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !predicate() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("worker progress deadline");
    }
    #[test]
    fn real_scheduler_idle_scan_retry_error_cancel_and_screen_refresh() {
        use std::sync::atomic::AtomicUsize;
        let (demo, receiver) = controller();
        let starts = AtomicUsize::new(0);
        let stops = AtomicUsize::new(0);
        let closes = AtomicUsize::new(0);
        let idle = AtomicUsize::new(0);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let worker = async {
                drive(&receiver, &demo.lifecycle, &demo.stop, async |command| {
                    match command {
                        Ok(Command::Scan(options)) => {
                            // Only the native-operation boundary is deterministic:
                            // a quiet advertisement stream, then stop or native error.
                            let operation = async {
                                let attempt = starts.fetch_add(1, Ordering::SeqCst) + 1;
                                demo.snapshot.lock().unwrap().scan_started(options.duration);
                                let _ = tokio::time::timeout(
                                    options.duration,
                                    futures::future::pending::<()>(),
                                )
                                .await;
                                stops.fetch_add(1, Ordering::SeqCst);
                                if attempt == 3 {
                                    return Err(Error::RuntimeError(
                                        "injected native scan failure".into(),
                                    ));
                                }
                                demo.snapshot.lock().unwrap().scan_completed(Vec::new());
                                Ok(())
                            };
                            if execute(
                                &demo.snapshot,
                                &demo.busy,
                                &demo.stop,
                                Some(&demo.lifecycle),
                                true,
                                operation,
                            )
                            .await
                            .is_some()
                            {
                                return Ok(false);
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            idle.fetch_add(1, Ordering::SeqCst);
                            let _ = tokio::time::timeout(
                                Duration::from_millis(25),
                                futures::future::pending::<()>(),
                            )
                            .await;
                        }
                        other => panic!("unexpected command boundary: {}", other.is_ok()),
                    }
                    Ok(true)
                })
                .await
                .unwrap();
                // Same awaited-close position as run(), after cancelling its operation.
                closes.fetch_add(1, Ordering::SeqCst);
                stops.fetch_add(1, Ordering::SeqCst);
                demo.busy.store(false, Ordering::SeqCst);
            };
            let ui = async {
                let mut feedback = Feedback::default();
                let epoch = feedback.start_polling().unwrap();
                until(|| idle.load(Ordering::SeqCst) > 0).await;
                for attempt in 1..=3 {
                    demo.send(Command::Scan(ScanOptions {
                        duration: Duration::from_millis(60),
                        ..Default::default()
                    }))
                    .unwrap();
                    until(|| starts.load(Ordering::SeqCst) == attempt).await;
                    assert!(demo.busy.load(Ordering::SeqCst));
                    assert_eq!(demo.snapshot().status, "Working");
                    feedback.message = demo
                        .send(Command::Scan(ScanOptions::default()))
                        .unwrap_err()
                        .to_string();
                    feedback.refresh(&demo.snapshot());
                    assert!(!feedback.message.is_empty());
                    // Exercise the actual screen refresh interval and retained outcome.
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    assert!(feedback.poll_is_current(epoch));
                    feedback.refresh(&demo.snapshot());
                    assert!(feedback.message.is_empty());
                    assert!(!demo.busy.load(Ordering::SeqCst));
                    assert_eq!(stops.load(Ordering::SeqCst), attempt);
                    if attempt < 3 {
                        assert!(feedback.scan_status.contains("0 matching devices"));
                    } else {
                        assert!(feedback
                            .scan_status
                            .contains("injected native scan failure"));
                    }
                }
                feedback.stop_polling();
                assert!(!feedback.poll_is_current(epoch));
                let new_epoch = feedback.start_polling().unwrap();
                assert!(feedback.poll_is_current(new_epoch));
                assert!(!feedback.poll_is_current(epoch));
                feedback.refresh(&demo.snapshot());
                assert!(feedback.scan_status.contains("failed"));
                demo.send(Command::Scan(ScanOptions {
                    duration: Duration::from_secs(2),
                    ..Default::default()
                }))
                .unwrap();
                until(|| starts.load(Ordering::SeqCst) == 4).await;
                demo.send(Command::Pause).unwrap();
                until(|| closes.load(Ordering::SeqCst) == 1).await;
                feedback.refresh(&demo.snapshot());
                assert!(feedback.scan_status.contains("app paused"));
                assert!(!demo.busy.load(Ordering::SeqCst));
                assert_eq!(stops.load(Ordering::SeqCst), 4);
            };
            futures::join!(worker, ui);
        });
    }
    #[test]
    fn empty_lifecycle_arbitration_consumes_each_accepted_command_once() {
        let (demo, receiver) = controller();
        for _ in 0..100 {
            assert!(matches!(
                next_command(&receiver, &demo.lifecycle),
                Err(mpsc::TryRecvError::Empty)
            ));
            demo.send(Command::Scan(ScanOptions::default())).unwrap();
            assert!(demo.send(Command::Scan(ScanOptions::default())).is_err());
            assert!(matches!(
                next_command(&receiver, &demo.lifecycle),
                Ok(Command::Scan(_))
            ));
            demo.busy.store(false, Ordering::SeqCst);
            assert!(matches!(
                next_command(&receiver, &demo.lifecycle),
                Err(mpsc::TryRecvError::Empty)
            ));
        }
    }
    #[test]
    fn zero_result_and_cancelled_scans_remain_distinct_after_ready() {
        let mut snapshot = Snapshot::default();
        snapshot.scan_started(Duration::from_secs(10));
        assert!(snapshot.scan_status.contains("10 seconds"));
        snapshot.scan_completed(Vec::new());
        snapshot.status = "Ready".into();
        assert!(snapshot.scan_status.contains("0 matching devices"));
        snapshot.scan_started(Duration::from_secs(10));
        snapshot.scan_cancelled("app paused during BLE operation");
        assert!(snapshot.scan_status.contains("app paused"));
        assert!(!snapshot.scan_status.contains("completed"));
    }
    #[test]
    fn rapid_pause_resume_preserves_fifo_before_worker_dequeue() {
        let (demo, receiver) = controller();
        demo.send(Command::Pause).unwrap();
        demo.resume(Permission::Granted);
        assert!(matches!(
            next_command(&receiver, &demo.lifecycle),
            Ok(Command::Pause)
        ));
        assert!(matches!(
            next_command(&receiver, &demo.lifecycle),
            Ok(Command::Resume(Permission::Granted))
        ));
        assert!(matches!(
            next_command(&receiver, &demo.lifecycle),
            Err(mpsc::TryRecvError::Empty)
        ));
    }
    #[test]
    fn resume_pause_preserves_permission_revocation_before_background_transition() {
        let (demo, receiver) = controller();
        demo.resume(Permission::Denied);
        demo.send(Command::Pause).unwrap();
        demo.resume(Permission::Granted);
        assert!(matches!(
            next_command(&receiver, &demo.lifecycle),
            Ok(Command::Resume(Permission::Denied))
        ));
        assert!(matches!(
            next_command(&receiver, &demo.lifecycle),
            Ok(Command::Pause)
        ));
        assert!(matches!(
            next_command(&receiver, &demo.lifecycle),
            Ok(Command::Resume(Permission::Granted))
        ));
    }
    #[test]
    fn resume_during_pause_cleanup_precedes_later_pause_and_user_work() {
        let (demo, receiver) = controller();
        demo.send(Command::Disconnect).unwrap();
        demo.send(Command::Pause).unwrap();
        assert!(matches!(
            next_command(&receiver, &demo.lifecycle),
            Ok(Command::Pause)
        ));
        // Worker holds the pause future; UI transitions arrive while busy.
        demo.busy.store(true, Ordering::SeqCst);
        demo.resume(Permission::Granted);
        demo.send(Command::Pause).unwrap();
        assert!(matches!(
            next_command(&receiver, &demo.lifecycle),
            Ok(Command::Resume(Permission::Granted))
        ));
        assert!(matches!(
            next_command(&receiver, &demo.lifecycle),
            Ok(Command::Pause)
        ));
        assert!(matches!(
            next_command(&receiver, &demo.lifecycle),
            Ok(Command::Disconnect)
        ));
        assert!(!demo.stop.load(Ordering::SeqCst));
    }
    #[test]
    fn lifecycle_overflow_cancels_instead_of_dropping_cleanup() {
        let (demo, _receiver) = controller();
        for _ in 0..8 {
            demo.send(Command::Pause).unwrap();
        }
        assert!(demo.send(Command::Pause).is_err());
        assert!(demo.stop.load(Ordering::SeqCst));
        assert_eq!(demo.lifecycle.lock().unwrap().len(), 8);
    }
    #[test]
    fn pause_interrupts_user_operation_but_not_lifecycle_cleanup() {
        let (demo, _receiver) = controller();
        demo.resume(Permission::Granted);
        assert!(!should_cancel(&demo.stop, Some(&demo.lifecycle)));
        demo.send(Command::Pause).unwrap();
        assert!(should_cancel(&demo.stop, Some(&demo.lifecycle)));
        assert!(!should_cancel(&demo.stop, None));
        demo.cancel();
        assert!(should_cancel(&demo.stop, None));
    }
    #[test]
    fn navigation_preserves_opt_in_but_explicit_close_always_cancels() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let demo = Demo {
            sender,
            snapshot: Arc::new(Mutex::new(Snapshot {
                background_enabled: true,
                ..Default::default()
            })),
            stop: Arc::new(AtomicBool::new(false)),
            busy: Arc::new(AtomicBool::new(false)),
            lifecycle: Arc::new(Mutex::new(VecDeque::new())),
        };
        demo.leave_screen();
        assert!(!demo.stop.load(Ordering::SeqCst));
        demo.resume(Permission::Granted);
        demo.resume(Permission::Denied);
        assert!(matches!(
            demo.lifecycle.lock().unwrap().back(),
            Some(Command::Resume(Permission::Denied))
        ));
        demo.cancel();
        assert!(demo.stop.load(Ordering::SeqCst));
        demo.stop.store(false, Ordering::SeqCst);
        demo.snapshot.lock().unwrap().background_enabled = false;
        demo.leave_screen();
        assert!(demo.stop.load(Ordering::SeqCst));
    }
    #[test]
    fn denied_permission_finishes_worker_without_opening_native_ble() {
        let demo = Demo::spawn(|| Err(Error::PermissionDenied)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !demo.snapshot().finished && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(demo.snapshot().finished);
        assert!(!demo.snapshot().status.is_empty());
        assert!(demo.send(Command::Disconnect).is_err());
    }
}
