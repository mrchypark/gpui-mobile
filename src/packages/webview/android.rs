use super::WebViewHandle;
use crate::android::platform_view::command;

pub fn evaluate_javascript(handle: &WebViewHandle, script: &str) -> Result<(), String> {
    command(super::active_id(handle)?, "webview.evaluate", &[script])
}
pub fn go_back(handle: &WebViewHandle) -> Result<(), String> {
    command(super::active_id(handle)?, "webview.back", &[])
}
pub fn reload(handle: &WebViewHandle) -> Result<(), String> {
    command(super::active_id(handle)?, "webview.reload", &[])
}
pub fn stop_loading(handle: &WebViewHandle) -> Result<(), String> {
    command(super::active_id(handle)?, "webview.stop", &[])
}
