use super::PackageInfo;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use std::ffi::CStr;

pub fn get_package_info() -> Result<PackageInfo, String> {
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        if bundle.is_null() {
            return Err("Failed to get NSBundle.mainBundle".into());
        }

        let app_name = bundle_string(bundle, "CFBundleDisplayName")
            .or_else(|| bundle_string(bundle, "CFBundleName"))
            .unwrap_or_default();

        let package_name =
            bundle_string(bundle, "CFBundleIdentifier").ok_or("Bundle identifier unavailable")?;

        let version = bundle_string(bundle, "CFBundleShortVersionString").unwrap_or_default();

        let build_number = bundle_string(bundle, "CFBundleVersion").unwrap_or_default();

        Ok(PackageInfo {
            app_name,
            package_name,
            version,
            build_number,
        })
    }
}

unsafe fn bundle_string(bundle: *mut AnyObject, key: &str) -> Option<String> {
    let key_nsstring = nsstring(key);
    let value: *mut AnyObject = msg_send![bundle, objectForInfoDictionaryKey: key_nsstring];
    if value.is_null() {
        return None;
    }
    let string: bool = msg_send![value, isKindOfClass: class!(NSString)];
    if !string {
        return None;
    }
    let utf8: *const i8 = msg_send![value, UTF8String];
    if utf8.is_null() {
        return None;
    }
    Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
}

use crate::ios::util::nsstring;
