//! Wake the active Android window's existing frame callback.

pub(crate) fn wake() {
    if let Some(window) = super::jni::platform().and_then(|platform| platform.primary_window()) {
        window.request_frame();
    }
}
