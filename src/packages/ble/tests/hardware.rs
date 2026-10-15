//! Opt-in test for a consenting echo peripheral; never executed by normal tests.
use super::*;
use futures::StreamExt;

#[test]
#[ignore = "requires BLE echo peripheral, granted OS permission, and BLE_ECHO_* environment"]
fn echo_gatt_reconnect_and_stale_disconnect() {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            let service = std::env::var("BLE_ECHO_SERVICE")
                .expect("BLE_ECHO_SERVICE")
                .parse()
                .unwrap();
            let characteristic = std::env::var("BLE_ECHO_CHARACTERISTIC")
                .expect("BLE_ECHO_CHARACTERISTIC")
                .to_ascii_lowercase();
            let expected = std::env::var("BLE_ECHO_TEXT")
                .expect("BLE_ECHO_TEXT")
                .into_bytes();
            let timeout = Duration::from_secs(20);
            let mut ble = Ble::new(Permission::Granted, timeout).await.unwrap();
            let result: Result<()> = async {
                let mut events = ble.events().await?;
                ble.start_scan(ScanFilter {
                    services: vec![service],
                })
                .await?;
                let id = deadline(timeout, async {
                    while let Some(event) = events.next().await {
                        if let CentralEvent::DeviceDiscovered(id) = event {
                            return Ok(id);
                        }
                    }
                    Err(Error::DeviceNotFound)
                })
                .await?;
                ble.stop_scan().await?;
                ble.connect(&id).await?;
                let services = ble.discover().await?;
                let c = services
                    .iter()
                    .flat_map(|s| &s.characteristics)
                    .find(|c| c.service_uuid == service && c.uuid.to_string() == characteristic)
                    .ok_or(Error::NoSuchCharacteristic)?
                    .clone();
                let mut values = ble.notifications().await?;
                ble.subscribe(&c).await?;
                ble.write(&c, &expected, WriteType::WithResponse).await?;
                let notification = deadline(timeout, async {
                    while let Some(value) = values.next().await {
                        if value.service_uuid == service && value.uuid.to_string() == characteristic
                        {
                            return Ok(value.value);
                        }
                    }
                    Err(Error::NotConnected)
                })
                .await?;
                assert_eq!(notification, expected);
                assert_eq!(ble.read(&c).await?, expected);
                ble.unsubscribe(&c).await?;
                // P1 regression: CoreBluetooth removed both native and Rust caches.
                // A failed/cancelled retrieval leaves exactly this state: no
                // session handle, but the caller still owns the retry target.
                ble.disconnect().await?;
                let fresh = ble.reconnect(&id).await?;
                let fresh = fresh
                    .iter()
                    .flat_map(|s| &s.characteristics)
                    .find(|c| c.service_uuid == service && c.uuid.to_string() == characteristic)
                    .ok_or(Error::NoSuchCharacteristic)?;
                // P2 regression: an old event must not poison the live connection.
                ble.observe(&CentralEvent::DeviceDisconnected(id)).await?;
                assert_eq!(ble.read(fresh).await?, expected);
                ble.disconnect().await?;
                Ok(())
            }
            .await;
            let cleanup = ble.close().await;
            assert!(cleanup.is_empty(), "cleanup failed: {cleanup:?}");
            result.unwrap();
        });
}
