//! Android task dispatcher.
//!
//! Mirrors the two-queue model used in `gpui_linux` and `IosDispatcher`:
//!
//! * **Foreground / main-thread tasks** — posted via a pipe-backed `ALooper`
//!   callback so they run on the Android native-activity main thread (the same
//!   thread that owns the `ANativeWindow` and processes input events).
//!
//! * **Background tasks** — dispatched onto a fixed-size Rust thread-pool
//!   backed by `std::thread`.  The pool size defaults to
//!   `std::thread::available_parallelism()`.
//!
//! ## Design notes
//!
//! `ALooper` (NDK) is the native equivalent of GCD's main queue or a Linux
//! `epoll` loop.  We register a pipe file-descriptor with `ALooper_addFd` and
//! write a single byte to wake it up whenever a foreground task is enqueued.
//! The looper callback drains all pending tasks before returning.
//!
//! ## GPUI integration
//!
//! `AndroidDispatcher` implements `gpui::PlatformDispatcher` so it can be
//! used to construct `BackgroundExecutor` and `ForegroundExecutor` instances.

#![allow(unsafe_code)]

use std::{
    collections::VecDeque,
    os::unix::io::RawFd,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use gpui::{PlatformDispatcher, Priority, RunnableVariant};
use parking_lot::Mutex;

// ── NDK / libc symbols we need ────────────────────────────────────────────────

/// `ALOOPER_POLL_CALLBACK` — the fd was signalled and the callback fired.
#[allow(dead_code)]
const ALOOPER_POLL_CALLBACK: i32 = -2;

/// `ALOOPER_EVENT_INPUT` — data is available to read on the fd.
const ALOOPER_EVENT_INPUT: i32 = 1;

unsafe extern "C" {
    /// Returns the looper associated with the calling thread, or null.
    fn ALooper_forThread() -> *mut libc_looper_opaque;
    /// Adds a file-descriptor to the looper.
    fn ALooper_addFd(
        looper: *mut libc_looper_opaque,
        fd: RawFd,
        ident: i32,
        events: i32,
        callback: Option<
            unsafe extern "C" fn(fd: RawFd, events: i32, data: *mut std::ffi::c_void) -> i32,
        >,
        data: *mut std::ffi::c_void,
    ) -> i32;
    /// Removes a file-descriptor from the looper.
    fn ALooper_removeFd(looper: *mut libc_looper_opaque, fd: RawFd) -> i32;
}

// Opaque C type placeholder.
#[repr(C)]
struct libc_looper_opaque {
    _priv: [u8; 0],
}

// ── task queue ────────────────────────────────────────────────────────────────

type BoxedTask = Box<dyn FnOnce() + Send + 'static>;

/// Shared state between the dispatcher and the looper callback.
struct MainQueue {
    tasks: VecDeque<BoxedTask>,
    /// Write end of the wake pipe.
    write_fd: RawFd,
}

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
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
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
                let _ = worker.join();
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
/// * Foreground tasks run on the Android main/native thread via `ALooper`.
/// * Background tasks run on a Rust thread-pool.
/// * A timer worker dispatches delayed tasks even when the main loop is idle.
pub struct AndroidDispatcher {
    /// Shared task queue + wake pipe for the main thread.
    main_queue: Arc<Mutex<MainQueue>>,
    /// Read end of the wake pipe (owned here for clean shutdown).
    read_fd: RawFd,
    /// Raw pointer to the main-thread `ALooper` (not `Send` — used only on the
    /// main thread).
    looper: *mut libc_looper_opaque,
    owner_thread: std::thread::ThreadId,
    /// Background thread-pool.
    pool: ThreadPool,
    timers: TimerQueue,
    /// Set to `true` once `shutdown()` is called.
    shutdown: AtomicBool,
}

// SAFETY: The `looper` pointer is only ever used on the main thread
// (in `register_with_looper` and `unregister`).  The rest of the fields are
// `Send`-safe via `Mutex` / `Arc`.
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
    /// Panics if:
    /// * `pipe(2)` fails.
    /// * `ALooper_forThread()` returns null (i.e. not called on main thread).
    pub fn new() -> Arc<Self> {
        // Create wake pipe.
        let (read_fd, write_fd) = create_pipe().expect("failed to create wake pipe");

        let looper = unsafe { ALooper_forThread() };
        assert!(
            !looper.is_null(),
            "AndroidDispatcher::new() must be called on the Android main thread"
        );

        let pool_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(2);

        let main_queue = Arc::new(Mutex::new(MainQueue {
            tasks: VecDeque::new(),
            write_fd,
        }));

        let pool = ThreadPool::new(pool_threads);
        let timers = TimerQueue::new(pool.sender.clone());
        let dispatcher = Arc::new(Self {
            main_queue: Arc::clone(&main_queue),
            read_fd,
            looper,
            owner_thread: std::thread::current().id(),
            pool,
            timers,
            shutdown: AtomicBool::new(false),
        });

        // Register the read end of the pipe with the looper so we get a
        // callback whenever a foreground task is enqueued.
        dispatcher.register_with_looper();

        log::debug!("AndroidDispatcher created (pool_threads={})", pool_threads);

        dispatcher
    }

    /// Create a dispatcher without a real `ALooper`.
    ///
    /// Safe to call from any thread (including non-Android host environments
    /// and unit tests).  Foreground tasks accumulate in the queue and must
    /// be drained manually via `flush_main_thread_tasks()`.
    pub fn new_headless() -> Arc<Self> {
        let (read_fd, write_fd) = create_pipe().expect("failed to create wake pipe");

        let pool_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .max(1);

        let main_queue = Arc::new(Mutex::new(MainQueue {
            tasks: VecDeque::new(),
            write_fd,
        }));

        let pool = ThreadPool::new(pool_threads);
        let timers = TimerQueue::new(pool.sender.clone());
        Arc::new(Self {
            main_queue,
            read_fd,
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
        if self.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let mut q = self.main_queue.lock();
        q.tasks.push_back(Box::new(f));
        wake_pipe(q.write_fd);
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

    /// Compatibility hook; deadlines no longer depend on event-loop polling.
    pub fn tick(&self) {}

    /// Drain all pending **main-thread** tasks synchronously.
    ///
    /// Useful in tests or when the looper callback is not set up.
    pub fn flush_main_thread_tasks(&self) {
        assert!(
            self.is_main_thread(),
            "foreground tasks must run on their owner thread"
        );
        loop {
            let task = {
                let mut q = self.main_queue.lock();
                q.tasks.pop_front()
            };
            match task {
                Some(f) => f(),
                None => break,
            }
        }
    }

    /// Stop accepting work and cancel pending timers. Already-running work may finish.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.timers.stop();
        self.unregister_from_looper();
    }

    // ── private helpers ───────────────────────────────────────────────────────

    fn register_with_looper(&self) {
        // We pass `main_queue` as the callback data pointer.  The callback
        // is a free `extern "C"` function that reconstructs the Arc from the
        // raw pointer.
        //
        // SAFETY: `self` outlives the looper registration because we hold an
        // `Arc<Self>` for the lifetime of the process (stored in
        // `GLOBAL_DISPATCHER`).
        let data_ptr = Arc::into_raw(Arc::clone(&self.main_queue)) as *mut std::ffi::c_void;

        let ret = unsafe {
            ALooper_addFd(
                self.looper,
                self.read_fd,
                0, // ident — unused when callback is Some
                ALOOPER_EVENT_INPUT,
                Some(looper_callback),
                data_ptr,
            )
        };

        if ret != 1 {
            unsafe {
                drop(Arc::from_raw(data_ptr as *const Mutex<MainQueue>));
            }
            log::warn!(
                "ALooper_addFd returned {} (expected 1); foreground dispatch may not work",
                ret
            );
        }
    }

    fn unregister_from_looper(&self) {
        if !self.looper.is_null() {
            unsafe {
                ALooper_removeFd(self.looper, self.read_fd);
            }
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
        self.shutdown.store(true, Ordering::SeqCst);
        self.unregister_from_looper();
        // Close file descriptors.
        unsafe {
            libc_close(self.read_fd);
            libc_close(self.main_queue.lock().write_fd);
        }
    }
}

// ── ALooper callback ──────────────────────────────────────────────────────────

/// Called by `ALooper` on the main thread when the wake pipe becomes readable.
///
/// Drains the entire task queue before returning 1 (keep the fd registered).
unsafe extern "C" fn looper_callback(fd: RawFd, _events: i32, data: *mut std::ffi::c_void) -> i32 {
    // Reconstruct the Arc without dropping it (we'll re-leak at the end).
    let queue_arc = unsafe { Arc::from_raw(data as *const Mutex<MainQueue>) };

    // Drain the wake pipe (ignore errors — just consume the byte(s)).
    let mut buf = [0u8; 64];
    loop {
        let n = unsafe { libc_read(fd, buf.as_mut_ptr() as *mut std::ffi::c_void, buf.len()) };
        if n <= 0 {
            break;
        }
    }

    // Run all queued tasks.
    loop {
        let task = {
            let mut q = queue_arc.lock();
            q.tasks.pop_front()
        };
        match task {
            Some(f) => f(),
            None => break,
        }
    }

    // Leak the Arc again so it isn't freed — the data pointer lifetime is
    // managed by `AndroidDispatcher`.
    std::mem::forget(queue_arc);

    1 // keep fd registered
}

// ── pipe helpers ──────────────────────────────────────────────────────────────

fn create_pipe() -> std::io::Result<(RawFd, RawFd)> {
    let mut fds = [0i32; 2];
    let ret = unsafe { libc_pipe(fds.as_mut_ptr()) };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Set both ends non-blocking so we never block in the callback.
    set_nonblocking(fds[0]);
    set_nonblocking(fds[1]);
    Ok((fds[0], fds[1]))
}

fn set_nonblocking(fd: RawFd) {
    unsafe {
        let flags = libc_fcntl(fd, LIBC_F_GETFL, 0);
        if flags >= 0 {
            libc_fcntl(fd, LIBC_F_SETFL, flags | LIBC_O_NONBLOCK);
        }
    }
}

/// Write a single wake byte to the pipe.
fn wake_pipe(write_fd: RawFd) {
    let buf = [1u8; 1];
    unsafe {
        libc_write(write_fd, buf.as_ptr() as *const std::ffi::c_void, 1);
    }
}

// ── minimal libc bindings ─────────────────────────────────────────────────────
// We declare only the symbols we actually use, keeping the dependency surface
// small.  The `libc` crate could be used here instead, but this avoids adding
// a workspace dependency just for this module.

const LIBC_F_GETFL: i32 = 3;
const LIBC_F_SETFL: i32 = 4;
const LIBC_O_NONBLOCK: i32 = 0o4000;

unsafe extern "C" {
    fn pipe(pipefd: *mut i32) -> i32;
    fn read(fd: i32, buf: *mut std::ffi::c_void, count: usize) -> isize;
    fn write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

#[inline(always)]
unsafe fn libc_pipe(fds: *mut i32) -> i32 {
    unsafe { pipe(fds) }
}
#[inline(always)]
unsafe fn libc_read(fd: i32, buf: *mut std::ffi::c_void, count: usize) -> isize {
    unsafe { read(fd, buf, count) }
}
#[inline(always)]
unsafe fn libc_write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize {
    unsafe { write(fd, buf, count) }
}
#[inline(always)]
unsafe fn libc_close(fd: i32) -> i32 {
    unsafe { close(fd) }
}
#[inline(always)]
unsafe fn libc_fcntl(fd: i32, cmd: i32, arg: i32) -> i32 {
    unsafe { fcntl(fd, cmd, arg) }
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
        if self.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let mut q = self.main_queue.lock();
        q.tasks.push_back(Box::new(move || {
            runnable.run();
        }));
        wake_pipe(q.write_fd);
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
        // Build a dispatcher without a real looper by constructing state manually.
        let (read_fd, write_fd) = create_pipe().unwrap();

        let main_queue = Arc::new(Mutex::new(MainQueue {
            tasks: VecDeque::new(),
            write_fd,
        }));

        let pool = ThreadPool::new(1);
        let timers = TimerQueue::new(pool.sender.clone());
        let dispatcher = AndroidDispatcher {
            main_queue,
            read_fd,
            looper: std::ptr::null_mut(),
            owner_thread: std::thread::current().id(),
            pool,
            timers,
            shutdown: AtomicBool::new(false),
        };

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

    /// Verify pipe creation succeeds.
    #[test]
    fn pipe_creation() {
        let (r, w) = create_pipe().expect("pipe");
        unsafe {
            libc_close(r);
            libc_close(w);
        }
    }
}
