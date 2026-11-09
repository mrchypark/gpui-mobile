//! Platform-neutral bookkeeping for frames that must bypass GPUI's view cache.
//!
//! GPUI only redraws when its invalidator is dirty. Some platform facts never
//! touch the GPUI entity system, so nothing marks the window dirty and the
//! change would stay invisible until some unrelated event woke the window:
//!
//! - window insets changed (rotation, split view, status bar, keyboard),
//! - the window appearance changed (light/dark),
//! - a native platform view was moved, resized, shown or hidden.
//!
//! `RequestFrameOptions::force_render` is the flag that answers this: it makes
//! the window refresh its render state and draw even when the invalidator is
//! clean. The flag is a *bypass of the cached view reuse*, not a "text input
//! arrived" marker, so it must be raised for every event that changes what
//! the platform draws.
//!
//! The flag is raised on whichever thread observes the change (the UI thread
//! for insets and appearance, the render thread for platform views) and
//! consumed on the render thread. The counter keeps the *number* of pending
//! requests so a diagnostic can tell "one thing changed" from "five things
//! changed", while the boolean that matters for rendering is the same as
//! GPUI's own `force_render_pending` flag: one pending request is enough to
//! bypass the cache, and further requests do not need their own frame.
//!
//! The counter is per-process rather than per-window because a window's frame
//! callback is the only place that can turn it into a `force_render`, and a
//! window that is not on screen has no frame callback to run.

use std::sync::atomic::{AtomicU64, Ordering};

/// The pending-request counter, isolated so tests can run in parallel
/// without stealing each other's requests.
#[derive(Debug, Default)]
struct ForceRender {
    pending: AtomicU64,
}

impl ForceRender {
    fn request(&self) -> bool {
        // `was_pending` tells the caller whether this request was the first
        // one, which is the only case that needs a host wake.
        self.pending.fetch_add(1, Ordering::AcqRel) == 0
    }

    fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) > 0
    }

    fn take(&self) -> bool {
        // `fetch_update` aborts with `Err` when the closure returns `None`,
        // which is the only way to tell "there was a request" from "the
        // counter was already zero": a `Some(count)` for a zero counter would
        // compare-exchange zero against zero, succeed, and report that a
        // request was consumed when none was.
        self.pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                if count == 0 {
                    None
                } else {
                    Some(count - 1)
                }
            })
            .is_ok()
    }

    fn drain(&self) -> usize {
        let mut count = 0;
        while self.take() {
            count += 1;
        }
        count
    }
}

/// Number of frames that must bypass GPUI's cached view reuse.
///
/// Incremented by [`request_force_render`] and drained by
/// [`take_force_render`]. Only the render thread drains it, so a raise that
/// arrives while a frame is being drawn is kept for the next frame instead
/// of being lost.
static FORCE_RENDER: ForceRender = ForceRender {
    pending: AtomicU64::new(0),
};

/// Ask for the next frame to bypass GPUI's cached view reuse.
///
/// Call this when a platform fact changed that GPUI cannot observe by
/// itself: window insets, window appearance, or the position, size or
/// visibility of a native platform view.
///
/// This is safe to call from any thread.
pub fn request_force_render() {
    if FORCE_RENDER.request() {
        // The window is probably idle: nothing asked it for a frame, so the
        // counter alone would never turn into a redraw. Ask the host for a
        // frame so the pending request can be consumed.
        wake_host();
    }
}

/// Whether a frame is waiting to bypass GPUI's cached view reuse.
///
/// This does not consume the request, so a caller can check it without
/// stealing the request from the frame that is about to draw.
pub fn force_render_pending() -> bool {
    FORCE_RENDER.is_pending()
}

/// Consume one pending force-render request.
///
/// Returns `true` when a request was pending. The render thread calls this
/// once per frame and passes the result as `force_render` to
/// `RequestFrameOptions`.
pub fn take_force_render() -> bool {
    FORCE_RENDER.take()
}

/// Ask the host to deliver a frame so a pending request is not stranded
/// while the window sits idle.
#[cfg(target_os = "android")]
fn wake_host() {
    crate::android::frame_source::wake();
}

/// Ask the host to deliver a frame so a pending request is not stranded
/// while the window sits idle.
#[cfg(target_os = "ios")]
fn wake_host() {
    crate::ios::ffi::wake_windows();
}

/// The desktop and test builds have no host loop to wake; a pending request
/// is picked up by the next frame the window asks for.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn wake_host() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn a_request_is_consumed_once() {
        let state = ForceRender::default();

        assert!(state.request());
        assert!(state.is_pending());
        assert!(state.take());
        assert!(!state.is_pending());
        assert!(!state.take());
    }

    #[test]
    fn two_requests_survive_one_frame() {
        let state = ForceRender::default();

        assert!(state.request());
        assert!(!state.request(), "the second request is not the first one");

        assert!(state.take());
        assert!(state.is_pending(), "the second request must survive");
        assert!(state.take());
        assert!(!state.take());
    }

    #[test]
    fn pending_does_not_consume() {
        let state = ForceRender::default();

        state.request();
        assert!(state.is_pending());
        assert!(state.is_pending());
        assert!(state.take());
        assert!(!state.is_pending());
    }

    #[test]
    fn requests_from_other_threads_are_counted() {
        let state = Arc::new(ForceRender::default());

        let threads: Vec<_> = (0..4)
            .map(|_| {
                let state = state.clone();
                thread::spawn(move || {
                    for _ in 0..64 {
                        state.request();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }

        let drained = state.drain();
        assert_eq!(drained, 4 * 64);
    }

    #[test]
    fn the_counter_does_not_underflow() {
        let state = ForceRender::default();

        // Saturating behaviour: a spurious extra drain must not wrap the
        // counter around to a huge value.
        assert!(!state.take());
        assert!(!state.is_pending());
    }

    #[test]
    fn concurrent_drains_consume_each_request_once() {
        let state = Arc::new(ForceRender::default());

        let requests = 256;
        for _ in 0..requests {
            state.request();
        }

        let barrier = Arc::new(std::sync::Barrier::new(4));
        let drains: Vec<_> = (0..4)
            .map(|_| {
                let barrier = barrier.clone();
                let state = state.clone();
                thread::spawn(move || {
                    barrier.wait();
                    state.drain()
                })
            })
            .collect();

        let total: usize = drains.into_iter().map(|d| d.join().unwrap()).sum();
        assert_eq!(total, requests);
    }

    #[test]
    fn only_the_first_request_needs_a_host_wake() {
        let state = ForceRender::default();

        assert!(state.request(), "the first request must ask for a frame");
        assert!(!state.request(), "a pending request already has a frame");
    }
}
