//! Embedded maps for Android and iOS.
//!
//! Provides a cross-platform embedded map view backed by:
//! - Android: Android MapView (via platform view system)
//! - iOS: MKMapView via Objective-C (via platform view system)
//!
//! Unlike `maps_launcher` which opens an external app, this package
//! embeds an interactive map directly in the GPUI render tree.
//!
//! Feature-gated behind `maps`.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
pub(crate) mod ios;

#[cfg(target_os = "android")]
use android as native;
#[cfg(target_os = "ios")]
use ios as native;

use crate::platform_view::{
    PlatformViewBounds, PlatformViewHandle, PlatformViewParams, PlatformViewRegistry,
};
use std::sync::Arc;

/// Geographic coordinate.
#[derive(Debug, Clone, Copy)]
pub struct LatLng {
    pub latitude: f64,
    pub longitude: f64,
}

/// Map type / style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapType {
    /// Standard road map.
    Normal,
    /// Satellite imagery.
    Satellite,
    /// Satellite with road overlays.
    Hybrid,
    /// Terrain/topographic.
    Terrain,
}

/// Configuration for creating a map view.
#[derive(Debug, Clone)]
pub struct MapSettings {
    /// Initial center coordinate.
    pub center: LatLng,
    /// Initial zoom level (0-20, higher = closer).
    pub zoom: f64,
    /// Map type/style.
    pub map_type: MapType,
    /// Enable zoom gestures.
    pub zoom_gestures_enabled: bool,
    /// Enable scroll/pan gestures.
    pub scroll_gestures_enabled: bool,
    /// Enable rotate gestures.
    pub rotate_gestures_enabled: bool,
    /// Show the user's location (requires permission).
    pub my_location_enabled: bool,
}

impl Default for MapSettings {
    fn default() -> Self {
        Self {
            center: LatLng {
                latitude: 37.7749,
                longitude: -122.4194,
            },
            zoom: 12.0,
            map_type: MapType::Normal,
            zoom_gestures_enabled: true,
            scroll_gestures_enabled: true,
            rotate_gestures_enabled: true,
            my_location_enabled: false,
        }
    }
}

/// A map marker/pin.
#[derive(Debug, Clone)]
pub struct MapMarker {
    /// Unique identifier for this marker.
    pub id: String,
    /// Position of the marker.
    pub position: LatLng,
    /// Title shown in the info window.
    pub title: Option<String>,
    /// Snippet/subtitle shown in the info window.
    pub snippet: Option<String>,
}

/// Handle to an embedded map view.
pub struct MapView {
    /// Platform view handle for the map.
    platform_handle: Option<Arc<PlatformViewHandle>>,
    /// Current settings.
    settings: MapSettings,
}

impl std::fmt::Debug for MapView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapView")
            .field("has_handle", &self.platform_handle.is_some())
            .field("center", &self.settings.center)
            .field("zoom", &self.settings.zoom)
            .finish()
    }
}

/// Register the "map" platform view factory.
fn ensure_factory_registered() {
    let registry = PlatformViewRegistry::global();
    if !registry.has_factory("map") {
        #[cfg(target_os = "android")]
        {
            use crate::android::platform_view::AndroidPlatformViewFactory;
            registry.register("map", Box::new(AndroidPlatformViewFactory::new("map")));
        }
        #[cfg(target_os = "ios")]
        {
            use crate::ios::platform_view::IosPlatformViewFactory;
            registry.register("map", Box::new(IosPlatformViewFactory::new("map")));
        }
    }
}

impl MapView {
    /// Create and display an embedded map view.
    pub fn new(settings: MapSettings) -> Result<Self, String> {
        validate_coordinate(settings.center)?;
        validate_zoom(settings.zoom)?;
        ensure_factory_registered();

        let mut creation_params = std::collections::HashMap::new();
        creation_params.insert("latitude".to_string(), settings.center.latitude.to_string());
        creation_params.insert(
            "longitude".to_string(),
            settings.center.longitude.to_string(),
        );
        creation_params.insert("zoom".to_string(), settings.zoom.to_string());
        creation_params.insert(
            "map_type".to_string(),
            (settings.map_type as i32).to_string(),
        );
        creation_params.insert(
            "zoom_gestures".to_string(),
            settings.zoom_gestures_enabled.to_string(),
        );
        creation_params.insert(
            "scroll_gestures".to_string(),
            settings.scroll_gestures_enabled.to_string(),
        );
        creation_params.insert(
            "rotate_gestures".to_string(),
            settings.rotate_gestures_enabled.to_string(),
        );
        creation_params.insert(
            "my_location".to_string(),
            settings.my_location_enabled.to_string(),
        );

        let params = PlatformViewParams {
            bounds: PlatformViewBounds::default(),
            creation_params,
        };

        let handle = PlatformViewRegistry::global().create_view("map", params)?;
        Ok(Self {
            platform_handle: Some(Arc::new(handle)),
            settings,
        })
    }

    /// Get the platform view handle for embedding in a GPUI element.
    pub fn platform_view_handle(&self) -> Option<Arc<PlatformViewHandle>> {
        self.platform_handle.clone()
    }

    /// Set the map center coordinate.
    pub fn set_center(&mut self, center: LatLng) -> Result<(), String> {
        validate_coordinate(center)?;
        let id = self.active_id()?;
        #[cfg(any(target_os = "ios", target_os = "android"))]
        native::set_center(id, center)?;
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        return Err(format!("Maps are unavailable on this platform ({id})"));
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            self.settings.center = center;
            Ok(())
        }
    }

    /// Set the zoom level.
    pub fn set_zoom(&mut self, zoom: f64) -> Result<(), String> {
        validate_zoom(zoom)?;
        let id = self.active_id()?;
        #[cfg(any(target_os = "ios", target_os = "android"))]
        native::set_zoom(id, zoom)?;
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        return Err(format!("Maps are unavailable on this platform ({id})"));
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            self.settings.zoom = zoom;
            Ok(())
        }
    }

    /// Set the map type/style.
    pub fn set_map_type(&mut self, map_type: MapType) -> Result<(), String> {
        let id = self.active_id()?;
        #[cfg(any(target_os = "ios", target_os = "android"))]
        native::set_map_type(id, map_type)?;
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = map_type;
            return Err(format!("Maps are unavailable on this platform ({id})"));
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            self.settings.map_type = map_type;
            Ok(())
        }
    }

    /// Add a marker to the map.
    pub fn add_marker(&self, marker: &MapMarker) -> Result<(), String> {
        validate_coordinate(marker.position)?;
        if marker.id.is_empty() {
            return Err("Marker ID must not be empty".into());
        }
        let id = self.active_id()?;
        #[cfg(target_os = "ios")]
        {
            ios::add_marker(id, marker)
        }
        #[cfg(target_os = "android")]
        {
            android::add_marker(id, marker)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = marker;
            Err(format!("Maps are unavailable on this platform ({id})"))
        }
    }

    /// Remove a marker by ID.
    pub fn remove_marker(&self, marker_id: &str) -> Result<(), String> {
        let id = self.active_id()?;
        #[cfg(target_os = "ios")]
        {
            ios::remove_marker(id, marker_id)
        }
        #[cfg(target_os = "android")]
        {
            android::remove_marker(id, marker_id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = marker_id;
            Err(format!("Maps are unavailable on this platform ({id})"))
        }
    }

    /// Remove all markers from the map.
    pub fn clear_markers(&self) -> Result<(), String> {
        let id = self.active_id()?;
        #[cfg(target_os = "ios")]
        {
            ios::clear_markers(id)
        }
        #[cfg(target_os = "android")]
        {
            android::clear_markers(id)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Err(format!("Maps are unavailable on this platform ({id})"))
        }
    }

    /// Dispose of the map view and release resources.
    pub fn dispose(&mut self) {
        if let Some(h) = self.platform_handle.take() {
            h.dispose();
        }
    }

    fn active_id(&self) -> Result<crate::platform_view::PlatformViewId, String> {
        let handle = self.platform_handle.as_ref().ok_or("Map is disposed")?;
        if handle.inner().is_disposed() || handle.view_type() != "map" {
            return Err("No active map instance".into());
        }
        Ok(handle.id())
    }
}

fn validate_coordinate(value: LatLng) -> Result<(), String> {
    if !value.latitude.is_finite()
        || !value.longitude.is_finite()
        || !(-90.0..=90.0).contains(&value.latitude)
        || !(-180.0..=180.0).contains(&value.longitude)
    {
        return Err("Coordinate must be finite, latitude -90..90 and longitude -180..180".into());
    }
    Ok(())
}

fn validate_zoom(value: f64) -> Result<(), String> {
    if !value.is_finite() || !(0.0..=20.0).contains(&value) {
        return Err("Zoom must be finite and within 0..20".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_camera_values_are_rejected() {
        assert!(validate_zoom(f64::NAN).is_err());
        assert!(validate_zoom(21.0).is_err());
        assert!(validate_zoom(0.0).is_ok());
        assert!(validate_coordinate(LatLng {
            latitude: 91.0,
            longitude: 0.0
        })
        .is_err());
        assert!(validate_coordinate(LatLng {
            latitude: 0.0,
            longitude: f64::INFINITY
        })
        .is_err());
    }
    #[test]
    fn disposed_map_rejects_every_control() {
        let mut map = MapView {
            platform_handle: None,
            settings: MapSettings::default(),
        };
        assert!(map.set_center(map.settings.center).is_err());
        assert!(map.set_zoom(1.0).is_err());
        assert_eq!(map.settings.zoom, 12.0);
        assert!(map.set_map_type(MapType::Hybrid).is_err());
        assert!(map.remove_marker("id").is_err());
        assert!(map.clear_markers().is_err());
        assert!(map
            .add_marker(&MapMarker {
                id: "id".into(),
                position: map.settings.center,
                title: None,
                snippet: None
            })
            .is_err());
    }
}

impl Drop for MapView {
    fn drop(&mut self) {
        self.dispose();
    }
}
