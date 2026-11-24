//! Android task dispatcher.
//!
//! Mirrors the two-queue model used in `gpui_linux` and `IosDispatcher`:
//!
//! * **Foreground / main-thread tasks** — queued for the Android
//!   native-activity main thread (the same thread that owns the
//!   `ANativeWindow` and processes input events), which runs them from its
//!   event loop.
//!
//! * **Background tasks** — dispatched onto a fixed-size Rust thread-pool
//!   backed by `std::thread`.  The pool size defaults to
//!   `std::thread::available_parallelism()`.
//!
//! ## Design notes
//!
//! `ALooper` (NDK) is the native equivalent of GCD's main queue or a Linux
//! `epoll` loop.  Enqueueing a foreground task calls `ALooper_wake`, so a main
//! loop blocked in `ALooper_pollOnce` returns, and the loop then drains the
//! queue with [`AndroidDispatcher::flush_main_thread_tasks`].
//!
//! The wake carries no callback on purpose. `android-activity` polls this
//! looper itself and logs every source it did not register as an error: a
//! looper callback comes back as `Spurious ALOOPER_POLL_CALLBACK`, and an fd
//! registered with an ident as `Ignoring spurious ALooper event source`.
//! `ALOOPER_POLL_WAKE` is the one result it accepts silently. Running the
//! tasks from the loop also gives them a fixed place in each iteration
//! instead of inside `android-activity`'s poll.
//!
//! ## GPUI integration
//!
//! `AndroidDispatcher` implements `gpui::PlatformDispatcher` so it can be
//! used to construct `BackgroundExecutor` and `ForegroundExecutor` instances.

#![allow(unsafe_code)]

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use gpui::{PlatformDispatcher, Priority, RunnableVariant};
use parking_lot::Mutex;

// ── NDK symbols we need ───────────────────────────────────────────────────────

unsafe extern "C" {
    /// Returns the looper associated with the calling thread, or null.
    fn ALooper_forThread() -> *mut libc_looper_opaque;
    fn ALooper_acquire(looper: *mut libc_looper_opaque);
    fn ALooper_release(looper: *mut libc_looper_opaque);
    /// Returns from a blocking poll on the looper; safe from any thread.
    fn ALooper_wake(looper: *mut libc_looper_opaque);
}

// Opaque C type placeholder.
#[repr(C)]
struct libc_looper_opaque {
    _priv: [u8; 0],
}

// ── task queue ────────────────────────────────────────────────────────────────

type BoxedTask = Box<dyn FnOnce() + Send + 'static>;

// ── thread-pool ───────────────────────────────────────────────────────────────

/// A minimal fixed-size thread-pool for background tasks.
struct ThreadPool {
    sender: std::sync::mpsc::Sender<BoxedTask>,
}

impl ThreadPool {
    fn new(threads: usize) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<BoxedTask>();
        let receiver = Arc::new(Mutex::new(receiver));

        for i in 0..threads {
            let rx = Arc::clone(&receiver);
            std::thread::Builder::new()
                .name(format!("gpui-bg-{}", i))
                .spawn(move || {
                    loop {
                        let task = {
                            let lock = rx.lock();
                            lock.recv()
                        };
                        match task {
                            Ok(f) => f(),
                            Err(_) => break, // channel closed
                        }
                    }
                })
                .expect("failed to spawn background thread");
        }

        ThreadPool { sender }
    }

    fn dispatch(&self, task: BoxedTask) {
        // If the pool is shutting down the send will fail silently.
        let _ = self.sender.send(task);
    }
}

// ── delayed task queue ────────────────────────────────────────────────────────

struct DelayedTask {
    due: Option<Instant>,
    task: BoxedTask,
}

struct TimerQueue {
    sender: Mutex<Option<std::sync::mpsc::Sender<DelayedTask>>>,
    worker: Mutex<Option<std::thread::JoinHandle<Vec<DelayedTask>>>>,
}

impl TimerQueue {
    fn new(background: std::sync::mpsc::Sender<BoxedTask>) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<DelayedTask>();
        let worker = std::thread::Builder::new()
            .name("gpui-timer".into())
            .spawn(move || {
                let mut pending: Vec<DelayedTask> = Vec::new();
                loop {
                    let received = match pending.first().and_then(|task| task.due) {
                        Some(due) => {
                            receiver.recv_timeout(due.saturating_duration_since(Instant::now()))
                        }
                        None => receiver
                            .recv()
                            .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected),
                    };
                    match received {
                        Ok(task) => {
                            pending.push(task);
                            pending.sort_by(|a, b| match (a.due, b.due) {
                                (Some(a), Some(b)) => a.cmp(&b),
                                (Some(_), None) => std::cmp::Ordering::Less,
                                (None, Some(_)) => std::cmp::Ordering::Greater,
                                (None, None) => std::cmp::Ordering::Equal,
                            });
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let now = Instant::now();
                    while pending
                        .first()
                        .and_then(|task| task.due)
                        .is_some_and(|due| due <= now)
                    {
                        let _ = background.send(pending.remove(0).task);
                    }
                }
                pending
            })
            .expect("failed to spawn timer thread");
        Self {
            sender: Mutex::new(Some(sender)),
            worker: Mutex::new(Some(worker)),
        }
    }

    fn dispatch(&self, delay: Duration, task: BoxedTask) {
        if let Some(sender) = self.sender.lock().as_ref() {
            // An unrepresentable deadline must never wrap into an immediate timer.
            let _ = sender.send(DelayedTask {
                due: Instant::now().checked_add(delay),
                task,
            });
        }
    }

    fn stop(&self) {
        self.sender.lock().take();
        let worker = self.worker.lock().take();
        if let Some(worker) = worker {
            if worker.thread().id() != std::thread::current().id() {
                // Delayed tasks can wake non-Send foreground futures when dropped.
                // Their cancellation must therefore happen on the owner thread.
                if let Ok(pending) = worker.join() {
                    drop(pending);
                }
            }
        }
    }
}

impl Drop for TimerQueue {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── AndroidDispatcher ─────────────────────────────────────────────────────────

/// GPUI dispatcher for Android.
///
/// * Foreground tasks run on the Android main/native thread, from the main
///   loop's call to `flush_main_thread_tasks()` after the looper wakes it.
/// * Background tasks run on a Rust thread-pool.
/// * A timer worker dispatches delayed tasks even when the main loop is idle.
pub struct AndroidDispatcher {
    /// Tasks waiting to run on the main thread.
    main_queue: Mutex<VecDeque<BoxedTask>>,
    /// Raw pointer to the main-thread `ALooper`: compared in
    /// `is_main_thread` / `ALooper_acquire` and woken (from any thread) by `wake_looper`.
    looper: *mut libc_looper_opaque,
    owner_thread: std::thread::ThreadId,
    /// Background thread-pool.
    pool: ThreadPool,
    timers: TimerQueue,
    /// Set to `true` once `shutdown()` is called.
    shutdown: AtomicBool,
}

// SAFETY: The `looper` pointer is acquired for the dispatcher's lifetime and
// passed to `ALooper_wake`, which is documented as safe from any thread.
// The rest of the fields are `Send`-safe via `Mutex`.
unsafe impl Send for AndroidDispatcher {}
unsafe impl Sync for AndroidDispatcher {}

impl AndroidDispatcher {
    /// Create a new dispatcher.
    ///
    /// Must be called on the **main thread** so that `ALooper_forThread()`
    /// returns a valid looper.
    ///
    /// # Panics
    ///
    /// Panics if `ALooper_forThread()` returns null (i.e. not called on main
    /// thread).
    pub fn new() -> Arc<Self> {
        let looper = unsafe { ALooper_forThread() };
        assert!(
            !looper.is_null(),
            "AndroidDispatcher::new() must be called on the Android main thread"
        );

        unsafe { ALooper_acquire(looper) };
        let pool_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(2);

        let pool = ThreadPool::new(pool_threads);
        let timers = TimerQueue::new(pool.sender.clone());
        let dispatcher = Arc::new(Self {
            main_queue: Mutex::new(VecDeque::new()),
            looper,
            owner_thread: std::thread::current().id(),
            pool,
            timers,
            shutdown: AtomicBool::new(false),
        });

        log::debug!("AndroidDispatcher created (pool_threads={})", pool_threads);

        dispatcher
    }

    /// Create a dispatcher without a real `ALooper`.
    ///
    /// Safe to call from any thread (including non-Android host environments
    /// and unit tests).  Foreground tasks accumulate in the queue and must
    /// be drained manually via `flush_main_thread_tasks()`.
    pub fn new_headless() -> Arc<Self> {
        let pool_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .max(1);

        let pool = ThreadPool::new(pool_threads);
        let timers = TimerQueue::new(pool.sender.clone());
        Arc::new(Self {
            main_queue: Mutex::new(VecDeque::new()),
            looper: std::ptr::null_mut(), // no real looper in headless mode
            owner_thread: std::thread::current().id(),
            pool,
            timers,
            shutdown: AtomicBool::new(false),
        })
    }

    // ── public API ────────────────────────────────────────────────────────────

    /// Returns `true` if the calling thread is the main/UI thread.
    pub fn is_main_thread(&self) -> bool {
        // Headless dispatchers also have an owner for foreground task affinity.
        std::thread::current().id() == self.owner_thread
    }

    /// Enqueue a task to run on the **main** (foreground) thread.
    pub fn dispatch_on_main_thread<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.push_main_task(Box::new(f));
    }

    /// Queues `task` for the main thread and wakes its loop. Wakes even when
    /// called on the main thread: a task queued outside
    /// `flush_main_thread_tasks` (from input or rendering) must not wait for
    /// an unrelated wakeup.
    fn push_main_task(&self, task: BoxedTask) {
        if self.shutdown.load(Ordering::Relaxed) {
            return;
        }
        self.main_queue.lock().push_back(task);
        self.wake_looper();
    }

    /// Interrupts a main loop blocked on the looper so it drains the main queue.
    fn wake_looper(&self) {
        if !self.looper.is_null() {
            // SAFETY: `ALooper_wake` is documented as safe from any thread,
            // and `self.looper` is retained for the dispatcher's lifetime.
            unsafe { ALooper_wake(self.looper) };
        }
    }

    /// Enqueue a task on the **background** thread-pool.
    pub fn dispatch<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if self.shutdown.load(Ordering::Relaxed) {
            return;
        }
        self.pool.dispatch(Box::new(f));
    }

    /// Enqueue a task on the **background** thread-pool after `delay`.
    pub fn dispatch_after<F>(&self, delay: Duration, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if self.shutdown.load(Ordering::Relaxed) {
            return;
        }
        self.timers.dispatch(delay, Box::new(f));
    }

    /// Delayed work is released by the timer thread, not by event-loop polling.
    pub fn next_delayed_due(&self) -> Option<Instant> {
        None
    }

    /// Compatibility hook; deadlines no longer depend on event-loop polling.
    pub fn tick(&self) {}

    /// Drain all pending **main-thread** tasks synchronously.
    ///
    /// The main loop calls this every time the looper wakes it. Tasks queued
    /// while it runs (including by the tasks themselves) run in the same call.
    pub fn flush_main_thread_tasks(&self) {
        assert!(
            self.is_main_thread(),
            "foreground tasks must run on their owner thread"
        );
        loop {
            // The guard is dropped before the task runs, so a task can queue
            // more tasks.
            let task = self.main_queue.lock().pop_front();
            match task {
                Some(f) => f(),
                None => break,
            }
        }
    }

    pub(crate) fn cancel_foreground_queue(&self) {
        assert!(self.is_main_thread());
        let tasks = std::mem::take(&mut *self.main_queue.lock());
        drop(tasks);
    }

    /// Stop accepting new tasks and signal the thread-pool to wind down.
    pub fn shutdown(&self) {
        if !self.shutdown.swap(true, Ordering::SeqCst) {
            self.timers.stop();
            let tasks = std::mem::take(&mut *self.main_queue.lock());
            drop(tasks);
        }
    }
}

impl Default for AndroidDispatcher {
    fn default() -> Self {
        panic!(
            "AndroidDispatcher must be constructed via `AndroidDispatcher::new()` \
             on the main thread"
        );
    }
}

impl Drop for AndroidDispatcher {
    fn drop(&mut self) {
        self.shutdown();
        if !self.looper.is_null() {
            unsafe { ALooper_release(self.looper) };
        }
    }
}

// ── impl PlatformDispatcher ───────────────────────────────────────────────────

impl PlatformDispatcher for AndroidDispatcher {
    fn is_main_thread(&self) -> bool {
        // Delegate to the existing `is_main_thread` method.
        AndroidDispatcher::is_main_thread(self)
    }

    fn dispatch(&self, runnable: RunnableVariant, _priority: Priority) {
        // All non-realtime background tasks go to the thread-pool.
        // Priority-based scheduling is not yet implemented; all tasks are
        // treated equally by the pool.
        self.dispatch(move || {
            runnable.run();
        });
    }

    fn dispatch_on_main_thread(&self, runnable: RunnableVariant, _priority: Priority) {
        self.push_main_task(Box::new(move || {
            runnable.run();
        }));
    }

    fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
        if self.shutdown.load(Ordering::Relaxed) {
            return;
        }
        self.dispatch_after(duration, move || {
            runnable.run();
        });
    }

    fn spawn_realtime(&self, f: Box<dyn FnOnce() + Send>) {
        // Spawn a dedicated thread for realtime (audio) tasks, matching the
        // behaviour of the Linux dispatcher.
        std::thread::Builder::new()
            .name("gpui-realtime".to_string())
            .spawn(move || {
                f();
            })
            .expect("failed to spawn realtime thread");
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    /// Verify that the thread-pool executes background tasks.
    #[test]
    fn background_tasks_run() {
        let pool = ThreadPool::new(2);
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..10 {
            let c = Arc::clone(&counter);
            pool.dispatch(Box::new(move || {
                c.fetch_add(1, Ordering::Relaxed);
            }));
        }

        // Give threads time to process.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(counter.load(Ordering::Relaxed), 10);
    }

    /// Verify that delayed tasks are not dispatched before their due time.
    #[test]
    fn delayed_tasks_not_early() {
        let dispatcher = AndroidDispatcher::new_headless();

        let ran = Arc::new(AtomicBool::new(false));
        let ran2 = Arc::clone(&ran);

        dispatcher.dispatch_after(Duration::from_secs(60), move || {
            ran2.store(true, Ordering::Relaxed);
        });

        dispatcher.tick();
        assert!(!ran.load(Ordering::Relaxed), "task should not run yet");

        drop(dispatcher);
    }

    #[test]
    fn headless_foreground_has_an_owner_and_shutdown_rejects_work() {
        let dispatcher = AndroidDispatcher::new_headless();
        assert!(dispatcher.is_main_thread());
        let other = dispatcher.clone();
        assert!(!std::thread::spawn(move || other.is_main_thread())
            .join()
            .unwrap());
        dispatcher.dispatch_on_main_thread(|| {});
        dispatcher.flush_main_thread_tasks();
        dispatcher.shutdown();
        dispatcher.dispatch_on_main_thread(|| panic!("accepted after shutdown"));
        dispatcher.flush_main_thread_tasks();
    }

    #[test]
    fn shutdown_releases_foreground_queue_once() {
        let dispatcher = AndroidDispatcher::new_headless();
        let token = Arc::new(());
        let queued = token.clone();
        dispatcher.dispatch_on_main_thread(move || drop(queued));
        assert_eq!(Arc::strong_count(&token), 2);
        dispatcher.shutdown();
        dispatcher.shutdown();
        assert_eq!(Arc::strong_count(&token), 1);
    }

    #[test]
    fn shutdown_drops_pending_timer_on_owner_thread() {
        struct DropThread(Arc<Mutex<Option<std::thread::ThreadId>>>);
        impl Drop for DropThread {
            fn drop(&mut self) {
                *self.0.lock() = Some(std::thread::current().id());
            }
        }
        let dispatcher = AndroidDispatcher::new_headless();
        let dropped = Arc::new(Mutex::new(None));
        let token = DropThread(dropped.clone());
        dispatcher.dispatch_after(Duration::from_secs(60), move || drop(token));
        dispatcher.shutdown();
        assert_eq!(*dropped.lock(), Some(std::thread::current().id()));
    }

    #[test]
    fn timers_fire_without_tick_and_long_deadlines_do_not_block_short_ones() {
        let pool = ThreadPool::new(1);
        let timers = TimerQueue::new(pool.sender.clone());
        timers.dispatch(Duration::MAX, Box::new(|| panic!("overflowed timer fired")));
        let (tx, rx) = std::sync::mpsc::channel();
        let start = Instant::now();
        timers.dispatch(
            Duration::from_millis(20),
            Box::new(move || {
                tx.send(Instant::now()).unwrap();
            }),
        );
        assert!(
            rx.recv_timeout(Duration::from_secs(2))
                .unwrap()
                .duration_since(start)
                >= Duration::from_millis(20)
        );
        timers.stop();
    }
}
