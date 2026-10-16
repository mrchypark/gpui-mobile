use super::{LatLng, MapMarker, MapType};
use crate::ios::platform_view::with_native;
use crate::platform_view::{PlatformViewId, PlatformViewParams};
use objc2::encode::{Encode, Encoding};
use objc2::{class, msg_send, runtime::AnyObject};

#[link(name = "MapKit", kind = "framework")]
#[link(name = "CoreLocation", kind = "framework")]
extern "C" {}

#[repr(C)]
#[derive(Clone, Copy)]
struct Coordinate {
    latitude: f64,
    longitude: f64,
}
unsafe impl Encode for Coordinate {
    const ENCODING: Encoding = Encoding::Struct("?", &[Encoding::Double, Encoding::Double]);
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Span {
    latitude: f64,
    longitude: f64,
}
unsafe impl Encode for Span {
    const ENCODING: Encoding = Encoding::Struct("?", &[Encoding::Double, Encoding::Double]);
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Region {
    center: Coordinate,
    span: Span,
}
unsafe impl Encode for Region {
    const ENCODING: Encoding = Encoding::Struct("?", &[Coordinate::ENCODING, Span::ENCODING]);
}
impl From<LatLng> for Coordinate {
    fn from(value: LatLng) -> Self {
        Self {
            latitude: value.latitude,
            longitude: value.longitude,
        }
    }
}

fn map_type(value: MapType) -> Result<usize, String> {
    match value {
        MapType::Normal => Ok(0),
        MapType::Satellite => Ok(1),
        MapType::Hybrid => Ok(2),
        MapType::Terrain => Err("MapKit does not expose an equivalent Terrain map type".into()),
    }
}

unsafe fn zoom(view: *mut AnyObject, value: f64) {
    let center: Coordinate = msg_send![view, centerCoordinate];
    // Public zoom is a tile-style scale; MapKit fits this angular region to the view.
    let span = 360.0 / 2.0_f64.powf(value);
    let region = Region {
        center,
        span: Span {
            latitude: span.min(180.0),
            longitude: span,
        },
    };
    let _: () = msg_send![view, setRegion: region, animated: false];
}

pub(crate) unsafe fn create(
    frame: crate::ios::cg_types::ObjcCGRect,
    params: &PlatformViewParams,
) -> Result<*mut AnyObject, String> {
    let p = &params.creation_params;
    let number = |key: &str| -> Result<f64, String> {
        p.get(key)
            .ok_or_else(|| format!("Missing map {key}"))?
            .parse()
            .map_err(|_| format!("Invalid map {key}"))
    };
    let center = LatLng {
        latitude: number("latitude")?,
        longitude: number("longitude")?,
    };
    super::validate_coordinate(center)?;
    let level = number("zoom")?;
    super::validate_zoom(level)?;
    let kind = match p.get("map_type").map(String::as_str) {
        Some("0") => MapType::Normal,
        Some("1") => MapType::Satellite,
        Some("2") => MapType::Hybrid,
        Some("3") => MapType::Terrain,
        _ => return Err("Invalid map type".into()),
    };
    let kind = map_type(kind)?;
    let location = p.get("my_location").is_some_and(|v| v == "true");
    if location {
        let status: i32 = msg_send![class!(CLLocationManager), authorizationStatus];
        if status != 3 && status != 4 {
            return Err("Map user location requires existing location authorization".into());
        }
    }
    let view: *mut AnyObject = msg_send![class!(MKMapView), alloc];
    let view: *mut AnyObject = msg_send![view, initWithFrame: frame];
    if view.is_null() {
        return Err("Failed to allocate MKMapView".into());
    }
    let _: () = msg_send![view, setMapType: kind];
    let _: () = msg_send![view, setCenterCoordinate: Coordinate::from(center), animated: false];
    zoom(view, level);
    let enabled = |key: &str| p.get(key).map_or(true, |v| v == "true");
    let _: () = msg_send![view, setZoomEnabled: enabled("zoom_gestures")];
    let _: () = msg_send![view, setScrollEnabled: enabled("scroll_gestures")];
    let _: () = msg_send![view, setRotateEnabled: enabled("rotate_gestures")];
    let _: () = msg_send![view, setShowsUserLocation: location];
    Ok(view)
}

pub fn set_center(id: PlatformViewId, center: LatLng) -> Result<(), String> {
    with_native(id, "map", |entry| unsafe {
        let _: () = msg_send![entry.pointer as *mut AnyObject, setCenterCoordinate: Coordinate::from(center), animated: false];
        Ok(())
    })
}
pub fn set_zoom(id: PlatformViewId, value: f64) -> Result<(), String> {
    with_native(id, "map", |entry| unsafe {
        zoom(entry.pointer as *mut AnyObject, value);
        Ok(())
    })
}
pub fn set_map_type(id: PlatformViewId, value: MapType) -> Result<(), String> {
    let kind = map_type(value)?;
    with_native(id, "map", |entry| unsafe {
        let _: () = msg_send![entry.pointer as *mut AnyObject, setMapType: kind];
        Ok(())
    })
}
pub fn add_marker(id: PlatformViewId, marker: &MapMarker) -> Result<(), String> {
    with_native(id, "map", |entry| unsafe {
        let view = entry.pointer as *mut AnyObject;
        let annotation: *mut AnyObject = msg_send![class!(MKPointAnnotation), new];
        if annotation.is_null() {
            return Err("Failed to allocate map annotation".into());
        }
        let _: () = msg_send![annotation, setCoordinate: Coordinate::from(marker.position)];
        if let Some(title) = &marker.title {
            let title = crate::ios::util::nsstring(title);
            let _: () = msg_send![annotation, setTitle: title];
        }
        if let Some(snippet) = &marker.snippet {
            let snippet = crate::ios::util::nsstring(snippet);
            let _: () = msg_send![annotation, setSubtitle: snippet];
        }
        let _: () = msg_send![view, addAnnotation: annotation];
        if let Some(old) = entry.markers.insert(marker.id.clone(), annotation as usize) {
            let _: () = msg_send![view, removeAnnotation: old as *mut AnyObject];
        }
        let _: () = msg_send![annotation, release];
        Ok(())
    })
}
pub fn remove_marker(id: PlatformViewId, marker_id: &str) -> Result<(), String> {
    with_native(id, "map", |entry| unsafe {
        let marker = entry.markers.remove(marker_id).ok_or("Unknown marker ID")?;
        let _: () =
            msg_send![entry.pointer as *mut AnyObject, removeAnnotation: marker as *mut AnyObject];
        Ok(())
    })
}
pub fn clear_markers(id: PlatformViewId) -> Result<(), String> {
    with_native(id, "map", |entry| unsafe {
        for (_, marker) in entry.markers.drain() {
            let _: () = msg_send![entry.pointer as *mut AnyObject, removeAnnotation: marker as *mut AnyObject];
        }
        Ok(())
    })
}
