use std::sync::OnceLock;

static INITIAL_LINK: OnceLock<Option<String>> = OnceLock::new();

pub fn get_initial_link() -> Result<Option<String>, String> {
    INITIAL_LINK
        .get()
        .cloned()
        .ok_or_else(|| "Host has not captured the initial launch URL".into())
}

/// Host launch hook: pass None for an ordinary launch; never call for warm URLs.
pub(crate) fn record_initial_link(url: Option<String>) {
    if INITIAL_LINK.set(url.clone()).is_ok() {
        if let Some(url) = url {
            super::LATEST_LINK.lock().unwrap().get_or_insert(url);
        }
    }
}

/// Called from the FFI layer when a URL is opened.
pub(crate) fn handle_open_url(url: String) {
    super::notify_deep_link(&url);
}
