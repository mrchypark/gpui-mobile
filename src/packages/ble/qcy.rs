//! Read-only candidates from OpenQCY-Desktop ee7e849 (community research, not a
//! vendor specification). Firmware variants may expose neither characteristic.
use super::*;
pub const SERVICE: &str = "0000a001-0000-1000-8000-00805f9b34fb";
pub const FIRMWARE: &str = "00000007-0000-1000-8000-00805f9b34fb";
pub const BATTERY: &str = "00000008-0000-1000-8000-00805f9b34fb";
#[derive(Clone, Debug, Default)]
pub struct Readout {
    /// None means absent or not advertised as readable. No command-write fallback.
    pub firmware: Option<Vec<u8>>,
    pub battery: Option<Vec<u8>>,
}
fn allowed(c: &Characteristic) -> bool {
    c.service_uuid.to_string() == SERVICE
        && [FIRMWARE, BATTERY].contains(&c.uuid.to_string().as_str())
        && c.properties.contains(CharPropFlags::READ)
}
/// Call only after the user selects the exact peer and connect/discover succeeds.
/// No scanning, connection, subscription, write, or proprietary initialization.
pub async fn read_known_values(ble: &mut Ble) -> Result<Readout> {
    let mut result = Readout::default();
    for id in [FIRMWARE, BATTERY] {
        let c = match ble.characteristic_by_uuid(SERVICE, id) {
            Ok(c) => c,
            Err(Error::NoSuchCharacteristic) => continue,
            Err(error) => return Err(error),
        };
        if !allowed(&c) { continue; }
        let bytes = ble.read(&c).await?;
        if id == FIRMWARE { result.firmware = Some(bytes); }
        else { result.battery = Some(bytes); }
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn qcy_allowlist_requires_service_identity_and_native_read_flag() {
        let mut c = Characteristic { service_uuid: SERVICE.parse().unwrap(), uuid: BATTERY.parse().unwrap(),
            properties: CharPropFlags::READ, descriptors: BTreeSet::new() };
        assert!(allowed(&c));
        c.properties = CharPropFlags::WRITE; assert!(!allowed(&c));
        c.properties = CharPropFlags::READ; c.service_uuid = FIRMWARE.parse().unwrap(); assert!(!allowed(&c));
        c.service_uuid = SERVICE.parse().unwrap(); c.uuid = SERVICE.parse().unwrap(); assert!(!allowed(&c));
    }
}
