use super::{LatLng, MapMarker, MapType};
use crate::android::platform_view::command;
use crate::platform_view::PlatformViewId;

pub fn set_center(id: PlatformViewId, center: LatLng) -> Result<(), String> {
    command(
        id,
        "map.center",
        &[&center.latitude.to_string(), &center.longitude.to_string()],
    )
}
pub fn set_zoom(id: PlatformViewId, zoom: f64) -> Result<(), String> {
    command(id, "map.zoom", &[&zoom.to_string()])
}
pub fn set_map_type(id: PlatformViewId, kind: MapType) -> Result<(), String> {
    command(id, "map.type", &[&(kind as i32).to_string()])
}
pub fn add_marker(id: PlatformViewId, marker: &MapMarker) -> Result<(), String> {
    command(
        id,
        "map.add",
        &[
            &marker.id,
            &marker.position.latitude.to_string(),
            &marker.position.longitude.to_string(),
            marker.title.as_deref().unwrap_or(""),
            marker.snippet.as_deref().unwrap_or(""),
        ],
    )
}
pub fn remove_marker(id: PlatformViewId, marker: &str) -> Result<(), String> {
    command(id, "map.remove", &[marker])
}
pub fn clear_markers(id: PlatformViewId) -> Result<(), String> {
    command(id, "map.clear", &[])
}
