use super::WebViewHandle;
use crate::ios::platform_view::with_native;
use objc2::{class, msg_send, runtime::AnyObject};

#[link(name = "WebKit", kind = "framework")]
extern "C" {}

pub fn evaluate_javascript(handle: &WebViewHandle, script: &str) -> Result<(), String> {
    let main: bool = unsafe { msg_send![class!(NSThread), isMainThread] };
    if main {
        return Err("Synchronous JavaScript evaluation must run off the iOS main thread".into());
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    with_native(super::active_id(handle)?, "webview", |entry| unsafe {
        let view = entry.pointer as *mut AnyObject;
        let config: *mut AnyObject = msg_send![view, configuration];
        let prefs: *mut AnyObject = msg_send![config, preferences];
        let enabled: bool = msg_send![prefs, javaScriptEnabled];
        if !enabled {
            return Err("JavaScript is disabled for this WebView".into());
        }
        let script = crate::ios::util::nsstring(script);
        let callback =
            block2::RcBlock::new(move |_value: *mut AnyObject, error: *mut AnyObject| {
                let result = if error.is_null() {
                    Ok(())
                } else {
                    let description: *mut AnyObject = msg_send![error, localizedDescription];
                    let text: *const std::ffi::c_char = msg_send![description, UTF8String];
                    Err(if text.is_null() {
                        "JavaScript evaluation failed".into()
                    } else {
                        std::ffi::CStr::from_ptr(text)
                            .to_string_lossy()
                            .into_owned()
                    })
                };
                let _ = sender.send(result);
            });
        let _: () = msg_send![view, evaluateJavaScript: script, completionHandler: &*callback];
        Ok(())
    })?;
    receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "JavaScript completion timed out or WebView was disposed".to_string())?
}
pub fn go_back(handle: &WebViewHandle) -> Result<(), String> {
    with_native(super::active_id(handle)?, "webview", |entry| unsafe {
        let view = entry.pointer as *mut AnyObject;
        let can_go_back: bool = msg_send![view, canGoBack];
        if !can_go_back {
            return Err("WebView has no back history".into());
        }
        let _: *mut AnyObject = msg_send![view, goBack];
        Ok(())
    })
}
pub fn reload(handle: &WebViewHandle) -> Result<(), String> {
    with_native(super::active_id(handle)?, "webview", |entry| unsafe {
        let _: *mut AnyObject = msg_send![entry.pointer as *mut AnyObject, reload];
        Ok(())
    })
}
pub fn stop_loading(handle: &WebViewHandle) -> Result<(), String> {
    with_native(super::active_id(handle)?, "webview", |entry| unsafe {
        let _: () = msg_send![entry.pointer as *mut AnyObject, stopLoading];
        Ok(())
    })
}
