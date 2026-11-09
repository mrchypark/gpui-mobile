use super::{BarometerData, SensorAvailability, SensorData};

extern "C" {
    fn gpui_sensors_available() -> u32;
    fn gpui_sensors_read(sensor: u32, values: *mut f64) -> bool;
}

pub fn available_sensors() -> SensorAvailability {
    let bits = unsafe { gpui_sensors_available() };
    SensorAvailability {
        accelerometer: bits & 1 != 0,
        gyroscope: bits & 2 != 0,
        magnetometer: bits & 4 != 0,
        barometer: bits & 8 != 0,
    }
}
fn read(sensor: u32) -> Option<[f64; 3]> {
    let mut values = [0.0; 3];
    unsafe { gpui_sensors_read(sensor, values.as_mut_ptr()) }.then_some(values)
}
fn axes(sensor: u32) -> Option<SensorData> {
    read(sensor).map(|[x, y, z]| SensorData { x, y, z })
}
pub fn accelerometer() -> Option<SensorData> {
    axes(0)
}
pub fn gyroscope() -> Option<SensorData> {
    axes(1)
}
pub fn magnetometer() -> Option<SensorData> {
    axes(2)
}
pub fn barometer() -> Option<BarometerData> {
    read(3).and_then(|values| super::pressure_from_kpa(values[0]))
}
