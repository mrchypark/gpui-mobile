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
//! iOS picker calls must run on the main thread and service its run loop until
//! completion; overlapping presentations return an error. Android calls must
//! run off the UI thread. iOS file selections are imported copies.

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
pub(crate) fn prompt_paths(options: gpui::PathPromptOptions) -> Result<Option<Vec<std::path::PathBuf>>, String> {
    if !options.files || options.directories {
        return Err("Mobile GPUI path prompts support files only; directory grants are not filesystem paths".into());
    }
    let files = if options.multiple {
        open_files(&OpenFileOptions::default())?
    } else {
        open_file(&OpenFileOptions::default())?.into_iter().collect()
    };
    if files.is_empty() { return Ok(None); }
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

pub(crate) fn prompt_new_path(_directory: &std::path::Path, _suggested_name: Option<&str>) -> Result<Option<std::path::PathBuf>, String> {
    Err("Mobile save dialogs require export contents or a ContentResolver URI; GPUI's filesystem PathBuf cannot safely represent that contract".into())
}
