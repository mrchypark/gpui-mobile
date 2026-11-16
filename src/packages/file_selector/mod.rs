//! File selector for picking files and directories.
//!
//! Provides a cross-platform file picker API backed by:
//! - iOS: `UIDocumentPickerViewController` via Objective-C
//! - Android: Storage Access Framework via JNI
//!
//! Inspired by [file_selector](https://pub.dev/packages/file_selector).
//!
//! Feature-gated behind `file_selector`.
//!
//! Synchronous iOS picker calls must run on the main thread and service its run
//! loop until completion. The internal GPUI bridge starts the picker on a main
//! queue turn and completes asynchronously. Overlapping presentations return an
//! error. Android calls must run off the UI thread. iOS file selections are
//! imported copies.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

/// A group of file type filters.
///
/// At least one of `extensions`, `mime_types`, or `utis` should be non-empty.
#[derive(Debug, Clone, Default)]
pub struct TypeGroup {
    /// Human-readable label for this group (e.g. "Images").
    pub label: String,
    /// File extensions without the dot (e.g. `["jpg", "png"]`).
    pub extensions: Vec<String>,
    /// MIME types (e.g. `["image/jpeg", "image/png"]`).
    pub mime_types: Vec<String>,
    /// iOS Uniform Type Identifiers (e.g. `["public.jpeg", "public.png"]`).
    /// If empty, `extensions` and `mime_types` are used to infer UTIs.
    pub utis: Vec<String>,
}

/// Options for opening a file.
#[derive(Debug, Clone, Default)]
pub struct OpenFileOptions {
    /// Accepted file type groups. Empty means all files.
    pub accept_type_groups: Vec<TypeGroup>,
    /// Initial directory to open the picker in (platform hint, may be ignored).
    pub initial_directory: Option<String>,
}

/// Options for getting a save path.
#[derive(Debug, Clone, Default)]
pub struct SaveFileOptions {
    /// Accepted file type groups.
    pub accept_type_groups: Vec<TypeGroup>,
    /// Initial directory (platform hint).
    pub initial_directory: Option<String>,
    /// Suggested file name.
    pub suggested_name: Option<String>,
}

/// A file selected by the user.
#[derive(Debug, Clone)]
pub struct SelectedFile {
    /// Absolute path or content URI of the file.
    pub path: String,
    /// Display name of the file.
    pub name: String,
}

/// Open a file picker to select a single file.
///
/// Returns `Ok(None)` if the user cancelled.
pub fn open_file(options: &OpenFileOptions) -> Result<Option<SelectedFile>, String> {
    #[cfg(target_os = "ios")]
    {
        ios::open_file(options)
    }
    #[cfg(target_os = "android")]
    {
        android::open_file(options)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = options;
        Err("file_selector is only available on iOS and Android".into())
    }
}

/// Open a file picker to select multiple files.
///
/// Returns an empty Vec if the user cancelled.
pub fn open_files(options: &OpenFileOptions) -> Result<Vec<SelectedFile>, String> {
    #[cfg(target_os = "ios")]
    {
        ios::open_files(options)
    }
    #[cfg(target_os = "android")]
    {
        android::open_files(options)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = options;
        Err("file_selector is only available on iOS and Android".into())
    }
}

/// Open a save dialog to get a file path from the user.
///
/// Returns `Ok(None)` if the user cancelled.
/// Android returns a content URI, which must be written via ContentResolver.
/// iOS returns an error: native export requires existing file contents, which
/// this path-only API does not accept.
pub fn get_save_path(options: &SaveFileOptions) -> Result<Option<String>, String> {
    #[cfg(target_os = "ios")]
    {
        ios::get_save_path(options)
    }
    #[cfg(target_os = "android")]
    {
        android::get_save_path(options)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = options;
        Err("file_selector is only available on iOS and Android".into())
    }
}

/// Open a directory picker.
///
/// Returns `Ok(None)` if the user cancelled.
pub fn get_directory_path(initial_directory: Option<&str>) -> Result<Option<String>, String> {
    #[cfg(target_os = "ios")]
    {
        ios::get_directory_path(initial_directory)
    }
    #[cfg(target_os = "android")]
    {
        android::get_directory_path(initial_directory)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = initial_directory;
        Err("file_selector is only available on iOS and Android".into())
    }
}

/// Internal GPUI adapter: return usable filesystem paths, never SAF URIs.
pub(crate) fn prompt_paths(
    options: gpui::PathPromptOptions,
) -> Result<Option<Vec<std::path::PathBuf>>, String> {
    if !options.files || options.directories {
        return Err("Mobile GPUI path prompts support files only; directory grants are not filesystem paths".into());
    }
    let files = if options.multiple {
        open_files(&OpenFileOptions::default())?
    } else {
        open_file(&OpenFileOptions::default())?
            .into_iter()
            .collect()
    };
    if files.is_empty() {
        return Ok(None);
    }
    let mut paths = Vec::with_capacity(files.len());
    for file in files {
        let path = std::path::PathBuf::from(file.path);
        if !path.is_absolute() || !path.is_file() {
            return Err("Picker did not return an imported filesystem file".into());
        }
        paths.push(path);
    }
    Ok(Some(paths))
}

#[cfg(target_os = "ios")]
pub(crate) type AsyncPromptResult = Result<Option<AsyncPromptPaths>, String>;

#[cfg(any(target_os = "ios", test))]
pub(crate) struct AsyncPromptPaths(Vec<std::path::PathBuf>);

#[cfg(any(target_os = "ios", test))]
impl AsyncPromptPaths {
    fn new(paths: Vec<std::path::PathBuf>) -> Self {
        Self(paths)
    }

    pub(crate) fn take(mut self) -> Vec<std::path::PathBuf> {
        std::mem::take(&mut self.0)
    }
}

#[cfg(any(target_os = "ios", test))]
impl Drop for AsyncPromptPaths {
    fn drop(&mut self) {
        discard_import_paths(&self.0);
    }
}

#[cfg(any(target_os = "ios", test))]
fn discard_import_paths(paths: &[std::path::PathBuf]) {
    #[cfg(target_os = "ios")]
    ios::discard_paths(paths);
    #[cfg(not(target_os = "ios"))]
    for path in paths {
        let _ = std::fs::remove_file(path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

#[cfg(target_os = "ios")]
struct QueuedPathPrompt {
    multiple: bool,
    sender: futures::channel::oneshot::Sender<AsyncPromptResult>,
}

#[cfg(target_os = "ios")]
fn finish_async_prompt(
    sender: &std::sync::Arc<
        std::sync::Mutex<Option<futures::channel::oneshot::Sender<AsyncPromptResult>>>,
    >,
    result: AsyncPromptResult,
) {
    let sender = sender.lock().ok().and_then(|mut sender| sender.take());
    let Some(sender) = sender else {
        return;
    };
    let _ = sender.send(result);
}

#[cfg(any(target_os = "ios", test))]
fn prompt_receiver_closed<T>(
    sender: &std::sync::Arc<std::sync::Mutex<Option<futures::channel::oneshot::Sender<T>>>>,
) -> bool {
    sender
        .lock()
        .ok()
        .and_then(|sender| sender.as_ref().map(|sender| sender.is_canceled()))
        .unwrap_or(true)
}

#[cfg(target_os = "ios")]
unsafe extern "C" fn start_async_prompt(context: *mut std::ffi::c_void) {
    let queued = Box::from_raw(context.cast::<QueuedPathPrompt>());
    let QueuedPathPrompt { multiple, sender } = *queued;
    if sender.is_canceled() {
        return;
    }
    let sender = std::sync::Arc::new(std::sync::Mutex::new(Some(sender)));
    let callback_sender = sender.clone();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let session = match ios::begin_prompt(&OpenFileOptions::default(), multiple) {
            Ok(session) => session,
            Err(error) => {
                finish_async_prompt(&callback_sender, Err(error));
                return;
            }
        };
        if prompt_receiver_closed(&callback_sender) {
            session.cancel("Document picker request was dropped".into());
            return;
        }
        let session_owner = std::sync::Arc::new(std::sync::Mutex::new(Some(session)));
        let worker_session = session_owner.clone();
        let worker_sender = callback_sender.clone();
        let worker = std::thread::Builder::new()
            .name("gpui-document-picker".into())
            .spawn(move || {
                let session = worker_session
                    .lock()
                    .ok()
                    .and_then(|mut session| session.take());
                let Some(session) = session else {
                    finish_async_prompt(
                        &worker_sender,
                        Err("Document picker session ownership was lost".into()),
                    );
                    return;
                };
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        match session.wait_cancellable(|| prompt_receiver_closed(&worker_sender)) {
                            ios::PromptWaitResult::Complete(result) => Some(result.map(|paths| {
                                (!paths.is_empty()).then(|| AsyncPromptPaths::new(paths))
                            })),
                            ios::PromptWaitResult::Cancelled => None,
                        }
                    }))
                    .unwrap_or_else(|_| Some(Err("Document picker worker panicked".into())));
                if let Some(result) = result {
                    finish_async_prompt(&worker_sender, result);
                }
            });
        if let Err(error) = worker {
            if let Some(session) = session_owner
                .lock()
                .ok()
                .and_then(|mut session| session.take())
            {
                session.cancel(format!("Start document picker worker: {error}"));
            }
            finish_async_prompt(
                &callback_sender,
                Err(format!("Start document picker worker: {error}")),
            );
        }
    }));
    if outcome.is_err() {
        finish_async_prompt(&sender, Err("Document picker startup panicked".into()));
    }
}

/// Start an iOS picker from a normal main-run-loop turn and return immediately.
/// The synchronous public API remains available for callers that require it.
#[cfg(target_os = "ios")]
pub(crate) fn prompt_paths_nonblocking(
    options: gpui::PathPromptOptions,
) -> futures::channel::oneshot::Receiver<AsyncPromptResult> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    if !options.files || options.directories {
        let _ = sender.send(Err("Mobile GPUI path prompts support files only; directory grants are not filesystem paths".into()));
        return receiver;
    }
    let queued = Box::new(QueuedPathPrompt {
        multiple: options.multiple,
        sender,
    });
    unsafe {
        crate::ios::dispatch_async_f(
            crate::ios::dispatch_get_main_queue(),
            Box::into_raw(queued).cast(),
            Some(start_async_prompt),
        );
    }
    receiver
}

#[cfg(target_os = "ios")]
pub(crate) fn discard_paths(paths: &[std::path::PathBuf]) {
    discard_import_paths(paths);
}

pub(crate) fn prompt_new_path(
    _directory: &std::path::Path,
    _suggested_name: Option<&str>,
) -> Result<Option<std::path::PathBuf>, String> {
    Err("Mobile save dialogs require export contents or a ContentResolver URI; GPUI's filesystem PathBuf cannot safely represent that contract".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_unconsumed_prompt_result_removes_imports() {
        let root = std::env::temp_dir().join(format!(
            "gpui-async-prompt-drop-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("import.txt");
        std::fs::write(&file, "owned").unwrap();

        let (sender, receiver) =
            futures::channel::oneshot::channel::<Result<Option<AsyncPromptPaths>, String>>();
        assert!(sender
            .send(Ok(Some(AsyncPromptPaths::new(vec![file]))))
            .is_ok());
        assert!(root.exists());
        drop(receiver);
        assert!(!root.exists());
    }

    #[test]
    fn receiver_drop_is_visible_before_picker_start() {
        let (sender, receiver) = futures::channel::oneshot::channel::<()>();
        let sender = std::sync::Arc::new(std::sync::Mutex::new(Some(sender)));
        assert!(!prompt_receiver_closed(&sender));
        drop(receiver);
        assert!(prompt_receiver_closed(&sender));
    }
}
