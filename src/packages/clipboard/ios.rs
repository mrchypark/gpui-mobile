use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

pub fn set_text(text: &str) -> Result<(), String> {
    crate::ios::platform_view::on_main(|| unsafe {
        let pasteboard: *mut AnyObject = msg_send![class!(UIPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            return Err("Failed to get UIPasteboard".into());
        }

        let ns_text: *mut AnyObject = msg_send![class!(NSString), alloc];
        let ns_text: *mut AnyObject = msg_send![ns_text,
            initWithBytes: text.as_ptr() as *const std::ffi::c_void,
            length: text.len(),
            encoding: 4u64  // NSUTF8StringEncoding
        ];
        if ns_text.is_null() {
            return Err("Failed to create NSString".into());
        }

        let _: () = msg_send![pasteboard, setString: ns_text];
        let _: () = msg_send![ns_text, release];
        Ok(())
    })
}

pub fn get_text() -> Result<Option<String>, String> {
    crate::ios::platform_view::on_main(|| unsafe {
        let pasteboard: *mut AnyObject = msg_send![class!(UIPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            return Err("Failed to get UIPasteboard".into());
        }

        let has: bool = msg_send![pasteboard, hasStrings];
        if !has {
            return Ok(None);
        }

        let ns_string: *mut AnyObject = msg_send![pasteboard, string];
        if ns_string.is_null() {
            return Ok(None);
        }

        let utf8: *const std::ffi::c_char = msg_send![ns_string, UTF8String];
        if utf8.is_null() {
            return Err("Clipboard text could not be converted to UTF-8".into());
        }

        let length: usize = msg_send![ns_string, lengthOfBytesUsingEncoding: 4usize];
        let bytes = std::slice::from_raw_parts(utf8 as *const u8, length);
        String::from_utf8(bytes.to_vec())
            .map(Some)
            .map_err(|e| e.to_string())
    })
}

pub fn has_text() -> Result<bool, String> {
    crate::ios::platform_view::on_main(|| unsafe {
        let pasteboard: *mut AnyObject = msg_send![class!(UIPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            return Err("Failed to get UIPasteboard".into());
        }

        let has: bool = msg_send![pasteboard, hasStrings];
        Ok(has)
    })
}
