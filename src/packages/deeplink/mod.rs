//! Deep linking — handle incoming URLs and universal links.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
pub(crate) mod ios;

use std::sync::Mutex;

/// Callback type for deep link events.
type DeepLinkCallback = Box<dyn Fn(&str) + Send + 'static>;

struct Handlers {
    callback: Option<DeepLinkCallback>,
    pending: std::collections::VecDeque<String>,
    delivering: bool,
}
static CALLBACK: Mutex<Handlers> = Mutex::new(Handlers {
    callback: None,
    pending: std::collections::VecDeque::new(),
    delivering: false,
});
static LATEST_LINK: Mutex<Option<String>> = Mutex::new(None);

/// Register a callback to be invoked when a deep link is received.
pub fn set_deep_link_handler(handler: impl Fn(&str) + Send + 'static) {
    CALLBACK.lock().unwrap().callback = Some(Box::new(handler));
}

/// Called internally when a deep link URL is received.
#[allow(dead_code)]
pub(crate) fn notify_deep_link(url: &str) {
    *LATEST_LINK.lock().unwrap() = Some(url.to_owned());
    let mut state = CALLBACK.lock().unwrap();
    state.pending.push_back(url.to_owned());
    if state.delivering {
        return;
    }
    state.delivering = true;
    loop {
        let Some(url) = state.pending.pop_front() else {
            state.delivering = false;
            return;
        };
        if let Some(callback) = state.callback.take() {
            drop(state);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(&url)));
            state = CALLBACK.lock().unwrap();
            if state.callback.is_none() {
                state.callback = Some(callback);
            }
            if result.is_err() {
                log::error!("Deep-link callback panicked");
            }
        }
    }
}

/// Get the initial deep link URL that launched the app (if any).
pub fn get_initial_link() -> Result<Option<String>, String> {
    #[cfg(target_os = "ios")]
    {
        ios::get_initial_link()
    }
    #[cfg(target_os = "android")]
    {
        android::get_initial_link()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        Ok(None)
    }
}

/// Get the latest deep link URL received while the app was running.
pub fn get_latest_link() -> Option<String> {
    LATEST_LINK.lock().unwrap().clone()
}

#[cfg(test)]
mod tests {
    #[test]
    fn callback_can_replace_itself_and_deliver_reentrantly() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let next = seen.clone();
        super::set_deep_link_handler(move |_| {
            let next = next.clone();
            super::set_deep_link_handler(move |url| next.lock().unwrap().push(url.to_owned()));
            super::notify_deep_link("gpui://second");
        });
        super::notify_deep_link("gpui://first");
        assert_eq!(&*seen.lock().unwrap(), &["gpui://second"]);
        assert_eq!(super::get_latest_link().as_deref(), Some("gpui://second"));
        super::CALLBACK.lock().unwrap().callback = None;
        *super::LATEST_LINK.lock().unwrap() = None;
    }
}
