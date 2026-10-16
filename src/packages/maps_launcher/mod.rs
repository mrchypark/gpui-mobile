//! Maps launcher for opening native maps applications.
//!
//! Provides a cross-platform API to launch maps apps backed by:
//! - Android: Geo intent (opens Google Maps or default maps app)
//! - iOS: Apple Maps URL scheme
//!
//! Feature-gated behind `maps_launcher`.
//! On iOS, blocking launch methods require a worker thread; availability queries
//! can be called on either thread.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

/// Launch maps app showing a specific coordinate.
///
/// If `label` is provided, it will be shown as a marker title at the location.
/// Returns `Ok(true)` if the maps app was opened successfully.
pub fn open_coordinates(
    latitude: f64,
    longitude: f64,
    label: Option<&str>,
) -> Result<bool, String> {
    validate_coordinates(latitude, longitude)?;
    #[cfg(target_os = "ios")]
    {
        ios::open_coordinates(latitude, longitude, label)
    }
    #[cfg(target_os = "android")]
    {
        android::open_coordinates(latitude, longitude, label)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = (latitude, longitude, label);
        Err("maps_launcher is only available on iOS and Android".into())
    }
}

/// Launch maps app with a search query (e.g. address or place name).
///
/// Returns `Ok(true)` if the maps app was opened successfully.
pub fn open_query(query: &str) -> Result<bool, String> {
    #[cfg(target_os = "ios")]
    {
        ios::open_query(query)
    }
    #[cfg(target_os = "android")]
    {
        android::open_query(query)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = query;
        Err("maps_launcher is only available on iOS and Android".into())
    }
}

/// Launch maps app showing directions from current location to a destination.
///
/// iOS accepts a destination label hint. Android navigation accepts coordinates
/// only and returns an error for a nonempty label; pass None there.
/// Returns `Ok(true)` if the maps app was opened successfully.
pub fn open_directions(
    dest_latitude: f64,
    dest_longitude: f64,
    dest_label: Option<&str>,
) -> Result<bool, String> {
    validate_coordinates(dest_latitude, dest_longitude)?;
    #[cfg(target_os = "ios")]
    {
        ios::open_directions(dest_latitude, dest_longitude, dest_label)
    }
    #[cfg(target_os = "android")]
    {
        android::open_directions(dest_latitude, dest_longitude, dest_label)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = (dest_latitude, dest_longitude, dest_label);
        Err("maps_launcher is only available on iOS and Android".into())
    }
}

/// Check if a maps app is available on the device.
///
/// Scheme visibility restrictions in the host manifest/Info.plist apply.
pub fn is_available() -> Result<bool, String> {
    #[cfg(target_os = "ios")]
    {
        ios::is_available()
    }
    #[cfg(target_os = "android")]
    {
        android::is_available()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        Err("maps_launcher is only available on iOS and Android".into())
    }
}

fn validate_coordinates(latitude: f64, longitude: f64) -> Result<(), String> {
    if !latitude.is_finite() || !longitude.is_finite()
        || !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return Err("Coordinates must be finite degrees within latitude/longitude bounds".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinate_boundary_validation() {
        assert!(validate_coordinates(-90.0, 180.0).is_ok());
        assert!(validate_coordinates(f64::NAN, 0.0).is_err());
        assert!(validate_coordinates(0.0, f64::INFINITY).is_err());
        assert!(validate_coordinates(90.1, 0.0).is_err());
        assert!(validate_coordinates(0.0, -180.1).is_err());
    }
}
