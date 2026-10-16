use super::NetworkInfo;
use crate::android::jni::{self as jni_helpers, get_string, JniExt};
use jni::objects::JValue;

pub fn get_network_info() -> Result<NetworkInfo, String> {
    jni_helpers::with_env(|env| {
        let result = (|| {
            let activity = jni_helpers::activity(env)?;
            let name = env.new_string("wifi").e()?;
            let manager = env
                .call_method(
                    &activity,
                    jni::jni_str!("getSystemService"),
                    jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
                    &[JValue::Object(&name)],
                )
                .and_then(|v| v.l())
                .e()?;
            if manager.is_null() {
                return Ok(NetworkInfo::default());
            }
            let wifi = env
                .call_method(
                    &manager,
                    jni::jni_str!("getConnectionInfo"),
                    jni::jni_sig!("()Landroid/net/wifi/WifiInfo;"),
                    &[],
                )
                .and_then(|v| v.l())
                .e()?;
            if wifi.is_null() {
                return Ok(NetworkInfo::default());
            }
            let ssid = env
                .call_method(
                    &wifi,
                    jni::jni_str!("getSSID"),
                    jni::jni_sig!("()Ljava/lang/String;"),
                    &[],
                )
                .and_then(|v| v.l())
                .e()?;
            let wifi_name = normalize_ssid(&get_string(env, &ssid));
            let bssid = env
                .call_method(
                    &wifi,
                    jni::jni_str!("getBSSID"),
                    jni::jni_sig!("()Ljava/lang/String;"),
                    &[],
                )
                .and_then(|v| v.l())
                .e()?;
            let bssid = get_string(env, &bssid);
            let wifi_bssid = (!bssid.is_empty() && bssid != "02:00:00:00:00:00").then_some(bssid);
            let ip = env
                .call_method(
                    &wifi,
                    jni::jni_str!("getIpAddress"),
                    jni::jni_sig!("()I"),
                    &[],
                )
                .and_then(|v| v.i())
                .e()? as u32;
            let wifi_ip = (ip != 0).then(|| std::net::Ipv4Addr::from(ip.to_le_bytes()).to_string());
            Ok(NetworkInfo {
                wifi_name,
                wifi_bssid,
                wifi_ip,
            })
        })();
        if result.is_err() {
            env.exception_clear();
        }
        result
    })
}

fn normalize_ssid(value: &str) -> Option<String> {
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    (!value.is_empty() && value != "<unknown ssid>").then(|| value.to_owned())
}
#[cfg(test)]
mod tests {
    #[test]
    fn only_the_native_quote_pair_is_removed() {
        assert_eq!(
            super::normalize_ssid("\"\"quoted\"\""),
            Some("\"quoted\"".into())
        );
        assert_eq!(super::normalize_ssid("<unknown ssid>"), None);
        assert_eq!(super::normalize_ssid("\"\""), None);
    }
}
