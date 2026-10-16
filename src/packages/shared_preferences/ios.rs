use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

pub struct IosSharedPreferences;

impl IosSharedPreferences {
    pub fn new() -> Self {
        Self
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        unsafe {
            let defaults = user_defaults();
            let ns_key = nsstring(key);
            let value: *mut AnyObject = msg_send![defaults, objectForKey: ns_key];
            if value.is_null() {
                None
            } else {
                let is_string: bool = msg_send![value, isKindOfClass: class!(NSString)];
                if !is_string {
                    return None;
                }
                nsstring_to_string(value)
            }
        }
    }

    pub fn set_string(&self, key: &str, value: &str) -> Result<(), String> {
        unsafe {
            let defaults = user_defaults();
            let ns_key = nsstring(key);
            let ns_value = nsstring(value);
            let _: () = msg_send![defaults, setObject: ns_value, forKey: ns_key];
            Ok(())
        }
    }

    pub fn get_int(&self, key: &str) -> Option<i64> {
        unsafe {
            let defaults = user_defaults();
            let ns_key = nsstring(key);
            let obj: *mut AnyObject = msg_send![defaults, objectForKey: ns_key];
            if obj.is_null() {
                None
            } else {
                let is_number: bool = msg_send![obj, isKindOfClass: class!(NSNumber)];
                if !is_number || CFGetTypeID(obj) == CFBooleanGetTypeID() {
                    return None;
                }
                let kind: *const i8 = msg_send![obj, objCType];
                if kind.is_null() || !b"csilqCSILQ".contains(&(*kind as u8)) {
                    return None;
                }
                let val: i64 = msg_send![defaults, integerForKey: ns_key];
                Some(val)
            }
        }
    }

    pub fn set_int(&self, key: &str, value: i64) -> Result<(), String> {
        unsafe {
            let defaults = user_defaults();
            let ns_key = nsstring(key);
            let _: () = msg_send![defaults, setInteger: value, forKey: ns_key];
            Ok(())
        }
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        unsafe {
            let defaults = user_defaults();
            let ns_key = nsstring(key);
            let obj: *mut AnyObject = msg_send![defaults, objectForKey: ns_key];
            if obj.is_null() {
                None
            } else {
                if CFGetTypeID(obj) != CFBooleanGetTypeID() {
                    return None;
                }
                let val: bool = msg_send![defaults, boolForKey: ns_key];
                Some(val)
            }
        }
    }

    pub fn set_bool(&self, key: &str, value: bool) -> Result<(), String> {
        unsafe {
            let defaults = user_defaults();
            let ns_key = nsstring(key);
            let _: () = msg_send![defaults, setBool: value, forKey: ns_key];
            Ok(())
        }
    }

    pub fn remove(&self, key: &str) -> Result<(), String> {
        unsafe {
            let defaults = user_defaults();
            let ns_key = nsstring(key);
            let _: () = msg_send![defaults, removeObjectForKey: ns_key];
            Ok(())
        }
    }

    pub fn clear(&self) -> Result<(), String> {
        unsafe {
            let defaults = user_defaults();
            let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
            let domain: *mut AnyObject = msg_send![bundle, bundleIdentifier];
            if domain.is_null() {
                return Err("No application preferences domain".into());
            }
            let _: () = msg_send![defaults, removePersistentDomainForName: domain];
            Ok(())
        }
    }

    pub fn contains_key(&self, key: &str) -> bool {
        unsafe {
            let defaults = user_defaults();
            let ns_key = nsstring(key);
            let obj: *mut AnyObject = msg_send![defaults, objectForKey: ns_key];
            !obj.is_null()
        }
    }
}

unsafe fn user_defaults() -> *mut AnyObject {
    msg_send![class!(NSUserDefaults), standardUserDefaults]
}

extern "C" {
    fn CFGetTypeID(value: *const AnyObject) -> usize;
    fn CFBooleanGetTypeID() -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_roundtrip_does_not_coerce_other_types() {
        let prefs = IosSharedPreferences::new();
        let key = format!("gpui_test_{}", std::process::id());
        prefs.set_string(&key, "42").unwrap();
        assert_eq!(prefs.get_int(&key), None);
        prefs.set_int(&key, i64::MAX).unwrap();
        assert_eq!(prefs.get_int(&key), Some(i64::MAX));
        assert_eq!(prefs.get_bool(&key), None);
        prefs.set_bool(&key, false).unwrap();
        assert_eq!(prefs.get_bool(&key), Some(false));
        assert_eq!(prefs.get_int(&key), None);
        prefs.remove(&key).unwrap();
        assert!(!prefs.contains_key(&key));
    }
}

use crate::ios::util::nsstring;

unsafe fn nsstring_to_string(ns: *mut AnyObject) -> Option<String> {
    let utf8: *const i8 = msg_send![ns, UTF8String];
    if utf8.is_null() {
        return None;
    }
    let length: usize = msg_send![ns, lengthOfBytesUsingEncoding: 4usize];
    String::from_utf8(std::slice::from_raw_parts(utf8 as *const u8, length).to_vec()).ok()
}
