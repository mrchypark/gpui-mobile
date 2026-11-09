//! Device sensor access — accelerometer, gyroscope, magnetometer, barometer.
//!
//! Readings start lazily and return `None` until native data arrives. Updates stop
//! in the background and resume on a subsequent foreground read. iOS pressure
//! requires `NSMotionUsageDescription` and motion authorization. Availability
//! describes hardware, not permission or whether a reading has arrived.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

/// A 3-axis sensor reading (x, y, z) in SI units.
///
/// - Accelerometer: m/s²
/// - Gyroscope: rad/s
/// - Magnetometer: µT (microtesla)
#[derive(Debug, Clone, Copy, Default)]
pub struct SensorData {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// Atmospheric pressure reading.
#[derive(Debug, Clone, Copy, Default)]
pub struct BarometerData {
    /// Pressure in hectopascals (hPa).
    pub pressure: f64,
}

#[cfg(any(target_os = "ios", test))]
fn pressure_from_kpa(pressure: f64) -> Option<BarometerData> {
    let pressure = pressure * 10.0;
    (pressure.is_finite() && pressure > 0.0).then_some(BarometerData { pressure })
}

/// Which sensor types are available on this device.
#[derive(Debug, Clone, Copy, Default)]
pub struct SensorAvailability {
    pub accelerometer: bool,
    pub gyroscope: bool,
    pub magnetometer: bool,
    pub barometer: bool,
}

/// Check which sensors are available on this device.
pub fn available_sensors() -> SensorAvailability {
    #[cfg(target_os = "ios")]
    {
        ios::available_sensors()
    }
    #[cfg(target_os = "android")]
    {
        android::available_sensors()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        SensorAvailability::default()
    }
}

/// Get the latest accelerometer reading (includes gravity).
pub fn accelerometer() -> Option<SensorData> {
    #[cfg(target_os = "ios")]
    {
        ios::accelerometer()
    }
    #[cfg(target_os = "android")]
    {
        android::accelerometer()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        None
    }
}

/// Get the latest gyroscope reading.
pub fn gyroscope() -> Option<SensorData> {
    #[cfg(target_os = "ios")]
    {
        ios::gyroscope()
    }
    #[cfg(target_os = "android")]
    {
        android::gyroscope()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        None
    }
}

/// Get the latest magnetometer reading.
pub fn magnetometer() -> Option<SensorData> {
    #[cfg(target_os = "ios")]
    {
        ios::magnetometer()
    }
    #[cfg(target_os = "android")]
    {
        android::magnetometer()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        None
    }
}

/// Get the latest barometer (atmospheric pressure) reading.
pub fn barometer() -> Option<BarometerData> {
    #[cfg(target_os = "ios")]
    {
        ios::barometer()
    }
    #[cfg(target_os = "android")]
    {
        android::barometer()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_motion_kpa_is_converted_to_hpa() {
        assert_eq!(pressure_from_kpa(101.325).unwrap().pressure, 1013.25);
    }

    #[test]
    fn invalid_pressure_is_not_a_sample() {
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(pressure_from_kpa(value).is_none());
        }
    }
}
