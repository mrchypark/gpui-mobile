use super::*;
use futures::StreamExt;
use std::collections::BTreeMap;

pub(super) fn absent_scan_errors() -> BoxStream<'static, i32> {
    // A missing optional channel must never win select with an immediate EOF.
    Box::pin(futures::stream::pending())
}

async fn next_scan_event(events: &mut BoxStream<'static, CentralEvent>,
    errors: &mut BoxStream<'static, i32>) -> Result<CentralEvent> {
    match futures::future::select(events.next(), errors.next()).await {
        futures::future::Either::Right((Some(code), _)) =>
            Err(Error::RuntimeError(format!("native scan failed: {code}"))),
        futures::future::Either::Right((None, _)) |
        futures::future::Either::Left((None, _)) =>
            Err(Error::RuntimeError("scan event stream closed".into())),
        futures::future::Either::Left((Some(event), _)) => Ok(event),
    }
}

/// A finite foreground scan. Service UUIDs are ORed; name/RSSI restrictions are
/// ANDed. Missing name/RSSI cannot satisfy an enabled restriction.
#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub filter: ScanFilter,
    pub duration: Duration,
    pub name_prefix: Option<String>,
    pub minimum_rssi: Option<i16>,
    /// Bounds retained results; scanning still runs for the requested duration.
    pub max_devices: usize,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self { filter: ScanFilter::default(), duration: Duration::from_secs(10),
            name_prefix: None, minimum_rssi: None, max_devices: 256 }
    }
}
impl ScanOptions {
    fn validate(&self) -> Result<()> {
        if self.duration.is_zero() || self.duration > Duration::from_secs(300)
            || self.max_devices == 0 || self.max_devices > 4096 {
            return Err(Error::NotSupported("scan needs 0 < duration <= 300s and 1..=4096 devices".into()));
        }
        Ok(())
    }
    fn matches(&self, properties: &PeripheralProperties) -> bool {
        (self.filter.services.is_empty() || self.filter.services.iter().any(|id| properties.services.contains(id)))
            && self.name_prefix.as_ref().is_none_or(|prefix| {
                properties.local_name.as_ref().or(properties.advertisement_name.as_ref())
                    .is_some_and(|name| name.starts_with(prefix))
            })
            && self.minimum_rssi.is_none_or(|minimum| properties.rssi.is_some_and(|rssi| rssi >= minimum))
    }
}

#[derive(Clone, Debug)]
pub struct ScanResult {
    pub id: PeripheralId,
    pub properties: PeripheralProperties,
}

fn update<K: Ord>(results: &mut BTreeMap<K, PeripheralProperties>, id: K,
    properties: PeripheralProperties, options: &ScanOptions) {
    if !options.matches(&properties) { results.remove(&id); return; }
    if results.contains_key(&id) || results.len() < options.max_devices {
        results.insert(id, properties);
    }
}

impl Ble {
    /// Collect current advertisement snapshots, deduplicated by native ID.
    /// Always attempts stop on normal/error return. If this future is cancelled,
    /// await stop_scan/close (or keep the runtime alive for Drop cleanup).
    /// Empty results mean no matching advertisements, not a radio failure.
    pub async fn scan_for(&mut self, options: ScanOptions) -> Result<Vec<ScanResult>> {
        options.validate()?;
        if self.scanning { return Err(Error::NotSupported("scan already active".into())); }
        let mut events = self.events().await?;
        let mut errors = self.scan_errors();
        let mut results = BTreeMap::new();
        let outcome = async {
            self.start_scan(options.filter.clone()).await?;
            // The outer deadline also bounds per-device property queries and radio checks.
            let collection = async {
                loop {
                    let event = match tokio::time::timeout(Duration::from_millis(200),
                        next_scan_event(&mut events, &mut errors)).await {
                        Ok(next) => next?,
                        Err(_) => { self.ready().await?; continue; }
                    };
                    self.observe(&event).await?;
                    match event {
                        CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id)
                        | CentralEvent::ManufacturerDataAdvertisement { id, .. }
                        | CentralEvent::ServiceDataAdvertisement { id, .. }
                        | CentralEvent::ServicesAdvertisement { id, .. }
                        | CentralEvent::RssiUpdate { id, .. } => {
                            if let Some(properties) = self.properties(&id).await? {
                                update(&mut results, id, properties, &options);
                            }
                        }
                        CentralEvent::StateUpdate(state) if state != CentralState::PoweredOn =>
                            return Err(Error::NotSupported("Bluetooth radio left powered-on state".into())),
                        _ => {}
                    }
                }
            };
            match tokio::time::timeout(options.duration, collection).await {
                Err(_) => Ok(()), // Completion of the requested collection window.
                Ok(result) => result,
            }
        }.await;
        let stopped = self.stop_scan().await;
        merge_cleanup(outcome, stopped)?;
        Ok(results.into_iter().map(|(id, properties)| ScanResult { id, properties }).collect())
    }

    /// Connect and discover; failed preparation attempts disconnect before returning.
    pub async fn connect_and_discover(&mut self, id: &PeripheralId) -> Result<BTreeSet<Service>> {
        if self.peripheral.is_some() {
            return Err(Error::NotSupported("disconnect the current peripheral first".into()));
        }
        let result = async { self.connect(id).await?; self.discover().await }.await;
        match result {
            Ok(services) => Ok(services),
            Err(error) => match self.disconnect().await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(Error::RuntimeError(format!("operation failed: {error}; cleanup failed: {cleanup}"))),
            },
        }
    }

    /// Resolve canonical hyphenated UUID strings against the current discovered
    /// snapshot. Service qualification is mandatory; UUID matching ignores case.
    pub fn characteristic_by_uuid(&self, service: &str, characteristic: &str) -> Result<Characteristic> {
        self.state.ready()?;
        self.discovered.iter().find(|c| c.service_uuid.to_string().eq_ignore_ascii_case(service)
            && c.uuid.to_string().eq_ignore_ascii_case(characteristic))
            .cloned().ok_or(Error::NoSuchCharacteristic)
    }

    /// Opens the stream before enabling the CCCD; filters by BOTH UUIDs.
    /// Dropping the stream does not unsubscribe. Explicitly unsubscribe/close.
    pub async fn subscribe_values(&mut self, characteristic: &Characteristic)
        -> Result<BoxStream<'static, ValueNotification>> {
        let values = self.notifications().await?;
        self.subscribe(characteristic).await?;
        let characteristic = characteristic.clone();
        Ok(Box::pin(values.filter(move |value| futures::future::ready(
            notification_matches(value, &characteristic)))))
    }
}

fn notification_matches(value: &ValueNotification, characteristic: &Characteristic) -> bool {
    value.service_uuid == characteristic.service_uuid && value.uuid == characteristic.uuid
}

fn merge_cleanup(result: Result<()>, cleanup: Result<()>) -> Result<()> {
    match (result, cleanup) {
        (Err(error), Err(cleanup)) => Err(Error::RuntimeError(format!("operation failed: {error}; cleanup failed: {cleanup}"))),
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

impl Drop for Ble {
    fn drop(&mut self) {
        if !self.scanning && self.peripheral.is_none() && self.background.is_none() { return; }
        let adapter = self.adapter.clone();
        let peripheral = self.peripheral.take();
        let scanning = self.scanning;
        let timeout = self.timeout;
        let background = self.background.take();
        // Never block a UI/drop thread or create another runtime. Runtime shutdown
        // may cancel this task; callers needing confirmed cleanup must await close.
        self.runtime.spawn(async move {
            let _background = background;
            if scanning && deadline(timeout, adapter.stop_scan()).await.is_err() {
                eprintln!("BLE Drop: scan cleanup failed; await close for actionable errors");
            }
            if let Some(peripheral) = peripheral {
                if deadline(timeout, peripheral.disconnect()).await.is_err() {
                    eprintln!("BLE Drop: connection cleanup failed; await close for actionable errors");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_error_channel_waits_for_live_events_and_preserves_primary_eof() {
        use futures::FutureExt;
        futures::executor::block_on(async {
            let (sender, receiver) = futures::channel::mpsc::unbounded();
            let mut events: BoxStream<'static, CentralEvent> = Box::pin(receiver);
            let mut errors = absent_scan_errors();
            assert!(next_scan_event(&mut events, &mut errors).now_or_never().is_none());
            sender.unbounded_send(CentralEvent::StateUpdate(CentralState::PoweredOn)).unwrap();
            assert!(matches!(next_scan_event(&mut events, &mut errors).await,
                Ok(CentralEvent::StateUpdate(CentralState::PoweredOn))));
            drop(sender);
            assert!(next_scan_event(&mut events, &mut errors).await.is_err());
            let mut live: BoxStream<'static, CentralEvent> = Box::pin(futures::stream::pending());
            let mut failures: BoxStream<'static, i32> = Box::pin(futures::stream::iter([5]));
            let error = next_scan_event(&mut live, &mut failures).await.unwrap_err();
            assert!(error.to_string().contains("native scan failed: 5"));
        });
    }
    #[test]
    fn filters_deduplicate_refresh_remove_and_bound_results() {
        let options = ScanOptions { name_prefix: Some("Echo".into()), minimum_rssi: Some(-60), max_devices: 1, ..Default::default() };
        let mut values = BTreeMap::new();
        let good = PeripheralProperties { local_name: Some("Echo one".into()), rssi: Some(-40), ..Default::default() };
        update(&mut values, 1, good.clone(), &options);
        update(&mut values, 2, good.clone(), &options);
        assert_eq!(values.len(), 1);
        let mut changed = good.clone(); changed.rssi = Some(-50);
        update(&mut values, 1, changed, &options);
        assert_eq!(values[&1].rssi, Some(-50));
        let mut weak = good; weak.rssi = None;
        update(&mut values, 1, weak, &options);
        assert!(values.is_empty());
        assert!(!options.matches(&PeripheralProperties::default()));
    }
    #[test]
    fn service_filter_and_notification_use_service_identity() {
        let a = "0000180f-0000-1000-8000-00805f9b34fb".parse().unwrap();
        let b = "0000180a-0000-1000-8000-00805f9b34fb".parse().unwrap();
        let c = Characteristic { service_uuid: a, uuid: a, properties: CharPropFlags::NOTIFY, descriptors: BTreeSet::new() };
        let mut value = ValueNotification { service_uuid: b, uuid: a, value: vec![1] };
        assert!(!notification_matches(&value, &c));
        value.service_uuid = a; assert!(notification_matches(&value, &c));
        let options = ScanOptions { filter: ScanFilter { services: vec![a] }, ..Default::default() };
        let mut properties = PeripheralProperties::default();
        properties.services = vec![b]; assert!(!options.matches(&properties));
        properties.services.push(a); assert!(options.matches(&properties));
    }
    #[test]
    fn invalid_limits_and_cleanup_errors_are_not_success() {
        let mut options = ScanOptions::default(); options.duration = Duration::ZERO;
        assert!(options.validate().is_err());
        options.duration = Duration::from_secs(1); options.max_devices = 0;
        assert!(options.validate().is_err());
        assert!(merge_cleanup(Ok(()), Err(Error::NotConnected)).is_err());
        let error = merge_cleanup(Err(Error::PermissionDenied), Err(Error::NotConnected)).unwrap_err();
        assert!(error.to_string().contains("cleanup failed"));
    }
}
