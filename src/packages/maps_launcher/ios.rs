#[path = "../url_launcher/ios.rs"]
mod launcher;

pub fn open_coordinates(
    latitude: f64,
    longitude: f64,
    label: Option<&str>,
) -> Result<bool, String> {
    let url = match label {
        Some(l) => format!(
            "http://maps.apple.com/?ll={},{}&q={}",
            latitude,
            longitude,
            percent_encode(l)
        ),
        None => format!("http://maps.apple.com/?ll={},{}", latitude, longitude),
    };
    open_url(&url)
}

pub fn open_query(query: &str) -> Result<bool, String> {
    let url = format!("http://maps.apple.com/?q={}", percent_encode(query));
    open_url(&url)
}

pub fn open_directions(
    dest_latitude: f64,
    dest_longitude: f64,
    dest_label: Option<&str>,
) -> Result<bool, String> {
    let url = match dest_label {
        Some(l) => format!(
            "http://maps.apple.com/?daddr={},{}&q={}&dirflg=d",
            dest_latitude,
            dest_longitude,
            percent_encode(l)
        ),
        None => format!(
            "http://maps.apple.com/?daddr={},{}&dirflg=d",
            dest_latitude, dest_longitude
        ),
    };
    open_url(&url)
}

pub fn is_available() -> Result<bool, String> {
    launcher::can_launch_url("maps://")
}

fn open_url(url: &str) -> Result<bool, String> {
    launcher::launch_url(url)
}
/// Simple percent-encoding for URL query parameters.
fn percent_encode(input: &str) -> String {
    let mut result = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => {
                result.push_str("%20");
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}
