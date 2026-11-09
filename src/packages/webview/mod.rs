//! In-app WebView for loading web content.
//!
//! Provides a cross-platform WebView API backed by:
//! - Android: `android.webkit.WebView` via JNI
//! - iOS: `WKWebView` via Objective-C
//!
//! Uses the platform view system for native view embedding.
//!
//! Feature-gated behind `webview`.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

use crate::platform_view::{
    PlatformViewBounds, PlatformViewHandle, PlatformViewParams, PlatformViewRegistry,
};
use std::sync::Arc;

/// Configuration for creating a WebView.
#[derive(Debug, Clone)]
pub struct WebViewSettings {
    /// Enable JavaScript execution (default: true).
    pub javascript_enabled: bool,
    /// User-agent string override (None = platform default).
    pub user_agent: Option<String>,
    /// Allow zoom gestures (default: true).
    pub zoom_enabled: bool,
    /// Enable DOM storage / localStorage (default: true).
    pub dom_storage_enabled: bool,
    /// Top offset in logical points — the WebView starts this many points
    /// below the top of the screen, leaving room for a GPUI-rendered app bar.
    /// Default: 0.0 (fullscreen).
    pub top_offset: f32,
}

impl Default for WebViewSettings {
    fn default() -> Self {
        Self {
            javascript_enabled: true,
            user_agent: None,
            zoom_enabled: true,
            dom_storage_enabled: true,
            top_offset: 0.0,
        }
    }
}

/// Register the "webview" platform view factory.
fn ensure_factory_registered() {
    let registry = PlatformViewRegistry::global();
    if !registry.has_factory("webview") {
        #[cfg(target_os = "android")]
        {
            use crate::android::platform_view::AndroidPlatformViewFactory;
            registry.register(
                "webview",
                Box::new(AndroidPlatformViewFactory::new("webview")),
            );
        }
        #[cfg(target_os = "ios")]
        {
            use crate::ios::platform_view::IosPlatformViewFactory;
            registry.register("webview", Box::new(IosPlatformViewFactory::new("webview")));
        }
    }
}

fn settings_to_creation_params(
    settings: &WebViewSettings,
) -> std::collections::HashMap<String, String> {
    let mut params = std::collections::HashMap::new();
    params.insert(
        "javascript_enabled".to_string(),
        settings.javascript_enabled.to_string(),
    );
    params.insert(
        "dom_storage_enabled".to_string(),
        settings.dom_storage_enabled.to_string(),
    );
    params.insert(
        "zoom_enabled".to_string(),
        settings.zoom_enabled.to_string(),
    );
    if let Some(ref ua) = settings.user_agent {
        params.insert("user_agent".to_string(), ua.clone());
    }
    params
}

/// Load a URL in a platform-native WebView.
///
/// Creates a platform view of type "webview" and loads the URL.
/// Returns a handle for controlling and dismissing the view.
pub fn load_url(url: &str, settings: &WebViewSettings) -> Result<WebViewHandle, String> {
    if url.trim().is_empty() {
        return Err("WebView URL must not be empty".into());
    }
    validate_settings(settings)?;
    ensure_factory_registered();

    let mut params_map = settings_to_creation_params(settings);
    params_map.insert("url".to_string(), url.to_string());

    let params = PlatformViewParams {
        bounds: PlatformViewBounds {
            x: 0.0,
            y: settings.top_offset,
            width: 0.0, // Will be set by platform_view_element
            height: 0.0,
        },
        creation_params: params_map,
    };

    let handle = PlatformViewRegistry::global().create_view("webview", params)?;
    Ok(WebViewHandle {
        platform_handle: Some(Arc::new(handle)),
    })
}

/// Load raw HTML content in a WebView.
pub fn load_html(html: &str, settings: &WebViewSettings) -> Result<WebViewHandle, String> {
    validate_settings(settings)?;
    ensure_factory_registered();

    let mut params_map = settings_to_creation_params(settings);
    params_map.insert("html".to_string(), html.to_string());

    let params = PlatformViewParams {
        bounds: PlatformViewBounds {
            x: 0.0,
            y: settings.top_offset,
            width: 0.0,
            height: 0.0,
        },
        creation_params: params_map,
    };

    let handle = PlatformViewRegistry::global().create_view("webview", params)?;
    Ok(WebViewHandle {
        platform_handle: Some(Arc::new(handle)),
    })
}

/// Evaluate JavaScript in an existing WebView and wait for its native completion.
///
/// Call from a worker thread: the synchronous API rejects native UI-thread calls
/// to avoid blocking callback delivery. Results are discarded; native execution
/// errors and a ten-second completion timeout are returned as errors. A timeout
/// does not undo script effects. This does not await JavaScript promises.
pub fn evaluate_javascript(handle: &WebViewHandle, script: &str) -> Result<(), String> {
    active_id(handle)?;
    #[cfg(target_os = "ios")]
    {
        ios::evaluate_javascript(handle, script)
    }
    #[cfg(target_os = "android")]
    {
        android::evaluate_javascript(handle, script)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = (handle, script);
        Err("webview is only available on iOS and Android".into())
    }
}

/// Navigate the WebView back one page in its history.
pub fn go_back(handle: &WebViewHandle) -> Result<(), String> {
    active_id(handle)?;
    #[cfg(target_os = "ios")]
    {
        ios::go_back(handle)
    }
    #[cfg(target_os = "android")]
    {
        android::go_back(handle)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = handle;
        Err("webview is only available on iOS and Android".into())
    }
}

/// Reload the current page in the WebView.
pub fn reload(handle: &WebViewHandle) -> Result<(), String> {
    active_id(handle)?;
    #[cfg(target_os = "ios")]
    {
        ios::reload(handle)
    }
    #[cfg(target_os = "android")]
    {
        android::reload(handle)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = handle;
        Err("webview is only available on iOS and Android".into())
    }
}

/// Stop loading the current page in the WebView.
pub fn stop_loading(handle: &WebViewHandle) -> Result<(), String> {
    active_id(handle)?;
    #[cfg(target_os = "ios")]
    {
        ios::stop_loading(handle)
    }
    #[cfg(target_os = "android")]
    {
        android::stop_loading(handle)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = handle;
        Err("webview is only available on iOS and Android".into())
    }
}

/// Dismiss / destroy a WebView.
pub fn dismiss(mut handle: WebViewHandle) -> Result<(), String> {
    if let Some(h) = handle.platform_handle.take() {
        h.dispose();
    }
    Ok(())
}

/// Opaque handle to a native WebView instance.
///
/// Wraps a platform view handle for lifecycle management.
#[derive(Debug)]
pub struct WebViewHandle {
    /// Platform view handle managing the native WebView.
    pub platform_handle: Option<Arc<PlatformViewHandle>>,
}

impl WebViewHandle {
    /// Get the platform view handle for embedding in a GPUI element.
    pub fn platform_view_handle(&self) -> Option<Arc<PlatformViewHandle>> {
        self.platform_handle.clone()
    }
}

impl Drop for WebViewHandle {
    fn drop(&mut self) {
        if let Some(h) = self.platform_handle.take() {
            h.dispose();
        }
    }
}

fn active_id(handle: &WebViewHandle) -> Result<crate::platform_view::PlatformViewId, String> {
    let handle = handle.platform_handle.as_ref().ok_or("No active WebView")?;
    if handle.inner().is_disposed() || handle.view_type() != "webview" {
        return Err("No active WebView instance".into());
    }
    Ok(handle.id())
}

fn validate_settings(settings: &WebViewSettings) -> Result<(), String> {
    if !settings.top_offset.is_finite() || settings.top_offset < 0.0 {
        return Err("WebView top offset must be finite and non-negative".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_view::{PlatformView, PlatformViewId};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn invalid_load_parameters_fail_before_native_creation() {
        assert!(load_url("", &WebViewSettings::default()).is_err());
        let settings = WebViewSettings {
            top_offset: f32::NAN,
            ..Default::default()
        };
        assert!(load_html("", &settings).is_err());
    }

    struct View {
        id: PlatformViewId,
        kind: &'static str,
        disposed: AtomicBool,
    }
    impl PlatformView for View {
        fn id(&self) -> PlatformViewId {
            self.id
        }
        fn view_type(&self) -> &str {
            self.kind
        }
        fn set_bounds(&self, _: PlatformViewBounds) {}
        fn set_visible(&self, _: bool) {}
        fn set_z_index(&self, _: i32) {}
        fn dispose(&self) {
            self.disposed.store(true, Ordering::Relaxed);
        }
        fn is_disposed(&self) -> bool {
            self.disposed.load(Ordering::Relaxed)
        }
    }
    fn handle(kind: &'static str) -> WebViewHandle {
        WebViewHandle {
            platform_handle: Some(Arc::new(PlatformViewHandle::new(Box::new(View {
                id: PlatformViewId::next(),
                kind,
                disposed: AtomicBool::new(false),
            })))),
        }
    }
    #[test]
    fn instances_and_external_disposal_are_independent() {
        let first = handle("webview");
        let second = handle("webview");
        assert_ne!(active_id(&first).unwrap(), active_id(&second).unwrap());
        first.platform_view_handle().unwrap().dispose();
        assert!(active_id(&first).is_err());
        assert!(active_id(&second).is_ok());
        assert!(evaluate_javascript(&first, "1").is_err());
        assert!(go_back(&first).is_err());
        assert!(reload(&first).is_err());
        assert!(stop_loading(&first).is_err());
        assert!(active_id(&handle("map")).is_err());
    }
}
