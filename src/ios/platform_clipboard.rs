//! Native multi-item clipboard. File providers transfer copies across sandboxes.
use super::super::util::nsstring;
use anyhow::{anyhow, bail, Result};
use futures::{channel::oneshot, FutureExt};
use gpui::{ClipboardEntry, ClipboardItem, ClipboardString, ExternalPaths, Image};
use objc2::{
    class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool},
};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};
#[path = "../clipboard_wire.rs"]
mod wire;
const TEXT: &str = "public.utf8-plain-text";
const META: &str = "dev.gpui.clipboard.string-metadata";
const FILE: &str = "dev.gpui.clipboard.external-file";
unsafe fn string(object: *mut AnyObject) -> Result<String> {
    if object.is_null() {
        bail!("Missing native string");
    }
    let bytes: *const u8 = msg_send![object, UTF8String];
    let len: usize = msg_send![object, lengthOfBytesUsingEncoding: 4usize];
    if len == 0 {
        return Ok(String::new());
    }
    if bytes.is_null() {
        bail!("Invalid native UTF-8");
    }
    Ok(std::str::from_utf8(std::slice::from_raw_parts(bytes, len))?.to_owned())
}
fn main_thread() -> Result<()> {
    let main: Bool = unsafe { msg_send![class!(NSThread), isMainThread] };
    if !main.as_bool() {
        return Err(gpui::ClipboardReadError::Unavailable.into());
    }
    Ok(())
}
unsafe fn register(provider: &AnyObject, uti: &str, bytes: Vec<u8>) {
    let block = block2::RcBlock::new(
        move |done: &block2::DynBlock<dyn Fn(*mut AnyObject, *mut AnyObject)>| -> *mut AnyObject {
            objc2::rc::autoreleasepool(|_| {
                let data: *mut AnyObject = msg_send![class!(NSData), dataWithBytes: bytes.as_ptr().cast::<std::ffi::c_void>(), length: bytes.len()];
                done.call((data, std::ptr::null_mut()));
            });
            std::ptr::null_mut()
        },
    );
    let _: () = msg_send![provider, registerDataRepresentationForTypeIdentifier: nsstring(uti), visibility: 0isize, loadHandler: &*block];
}
unsafe fn provider() -> Result<Retained<AnyObject>> {
    let object: *mut AnyObject = msg_send![class!(NSItemProvider), new];
    Retained::from_raw(object).ok_or_else(|| anyhow!("Cannot create clipboard provider"))
}
pub(super) fn write(item: &ClipboardItem) -> Result<()> {
    main_thread()?;
    objc2::rc::autoreleasepool(|_| unsafe {
        let providers: *mut AnyObject = msg_send![class!(NSMutableArray), array];
        // Snapshot files before giving providers a lazy source URL. Failed
        // publication reclaims only files created by this call.
        struct Staging {
            paths: Vec<PathBuf>,
            published: bool,
        }
        impl Drop for Staging {
            fn drop(&mut self) {
                if !self.published {
                    for path in &self.paths {
                        let _ = std::fs::remove_file(path);
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::remove_dir(parent);
                        }
                    }
                }
            }
        }
        let mut staging = Staging {
            paths: Vec::new(),
            published: false,
        };
        for entry in &item.entries {
            match entry {
                ClipboardEntry::String(value) => {
                    let provider = provider()?;
                    register(&provider, TEXT, value.text.as_bytes().to_vec());
                    if let Some(metadata) = &value.metadata {
                        register(&provider, META, metadata.as_bytes().to_vec());
                    }
                    let _: () = msg_send![providers, addObject: &*provider];
                }
                ClipboardEntry::Image(image) => {
                    let provider = provider()?;
                    let (_, _, uti) = wire::FORMATS
                        .iter()
                        .find(|(format, _, _)| *format == image.format)
                        .ok_or_else(|| anyhow!("Unsupported clipboard image format"))?;
                    register(&provider, uti, image.bytes.clone());
                    let _: () = msg_send![providers, addObject: &*provider];
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    for path in &paths.0 {
                        if !path.is_file() {
                            bail!("Clipboard path is not a regular file");
                        }
                        let root = std::env::temp_dir().join("gpui-clipboard");
                        std::fs::create_dir_all(&root)?;
                        let unique = format!(
                            "{}-{}-{}",
                            std::process::id(),
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)?
                                .as_nanos(),
                            staging.paths.len()
                        );
                        let directory = root.join(unique);
                        std::fs::create_dir(&directory)?;
                        let snapshot = directory.join(
                            path.file_name()
                                .ok_or_else(|| anyhow!("Clipboard file has no name"))?,
                        );
                        staging.paths.push(snapshot.clone());
                        std::fs::copy(path, &snapshot)?;
                        let path = snapshot
                            .to_str()
                            .ok_or_else(|| anyhow!("Clipboard path is not UTF-8"))?;
                        let url: *mut AnyObject =
                            msg_send![class!(NSURL), fileURLWithPath: nsstring(path)];
                        let object: *mut AnyObject = msg_send![class!(NSItemProvider), alloc];
                        let object: *mut AnyObject = msg_send![object, initWithContentsOfURL: url];
                        let provider = Retained::from_raw(object)
                            .ok_or_else(|| anyhow!("Cannot publish clipboard file"))?;
                        register(&provider, FILE, vec![1]);
                        let _: () = msg_send![providers, addObject: &*provider];
                    }
                }
            }
        }
        let pasteboard: *mut AnyObject = msg_send![class!(UIPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            bail!("Pasteboard unavailable");
        }
        let _: () = msg_send![pasteboard, setItemProviders: providers];
        staging.published = true;
        Ok(())
    })
}
// Keep the native operation alive until completion; dropping a pending future
// requests cancellation. Providers may still call back after cancellation.
struct PendingProgress(Option<Retained<AnyObject>>);
async unsafe fn completion<T>(
    receiver: oneshot::Receiver<Result<T>>,
    native_progress: *mut AnyObject,
) -> Result<T> {
    let mut progress = PendingProgress(Retained::retain(native_progress));
    let result = receiver
        .await
        .map_err(|_| anyhow!("Clipboard provider disconnected"))?;
    progress.0 = None;
    result
}
impl Drop for PendingProgress {
    fn drop(&mut self) {
        if let Some(progress) = &self.0 {
            unsafe {
                let _: () = msg_send![&**progress, cancel];
            }
        }
    }
}
struct ImportedFile(PathBuf);
impl Drop for ImportedFile {
    fn drop(&mut self) {
        if self.0.as_os_str().is_empty() {
            return;
        }
        let _ = std::fs::remove_file(&self.0);
        if let Some(parent) = self.0.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}
unsafe fn native_error(error: *mut AnyObject) -> anyhow::Error {
    let description: *mut AnyObject = msg_send![error, localizedDescription];
    let message = string(description).unwrap_or_else(|_| "Native clipboard provider failed".into());
    let domain: *mut AnyObject = msg_send![error, domain];
    let code: isize = msg_send![error, code];
    if code == 257 && string(domain).ok().as_deref() == Some("NSCocoaErrorDomain") {
        return gpui::ClipboardReadError::Denied(message).into();
    }
    anyhow!(message)
}
unsafe fn has(provider: &AnyObject, uti: &str) -> bool {
    let result: Bool = msg_send![provider, hasItemConformingToTypeIdentifier: nsstring(uti)];
    result.as_bool()
}
async unsafe fn load(provider: &AnyObject, uti: &str) -> Result<Vec<u8>> {
    let (tx, rx) = oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let block = block2::RcBlock::new(move |data: *mut AnyObject, error: *mut AnyObject| {
        let result = (|| {
            if !error.is_null() {
                return Err(native_error(error));
            }
            if data.is_null() {
                bail!("Clipboard provider returned no data");
            }
            let length: usize = msg_send![data, length];
            let bytes: *const std::ffi::c_void = msg_send![data, bytes];
            if length == 0 {
                return Ok(Vec::new());
            }
            if bytes.is_null() {
                bail!("Clipboard provider returned invalid data");
            }
            Ok(std::slice::from_raw_parts(bytes.cast::<u8>(), length).to_vec())
        })();
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(result);
        }
    });
    let progress: *mut AnyObject = msg_send![provider, loadDataRepresentationForTypeIdentifier: nsstring(uti), completionHandler: &*block];
    completion(rx, progress).await
}
async unsafe fn import_file(provider: &AnyObject, uti: &str) -> Result<ImportedFile> {
    let suggested: *mut AnyObject = msg_send![provider, suggestedName];
    let suggested = string(suggested).unwrap_or_else(|_| "clipboard-file".into());
    let name = Path::new(&suggested)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("clipboard-file")
        .to_owned();
    let (tx, rx) = oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let block = block2::RcBlock::new(move |url: *mut AnyObject, error: *mut AnyObject| {
        let result = (|| -> Result<ImportedFile> {
            if !error.is_null() {
                return Err(native_error(error));
            }
            if url.is_null() {
                bail!("Clipboard provider returned no file");
            }
            let path: *mut AnyObject = msg_send![url, path];
            let source = PathBuf::from(string(path)?);
            unsafe extern "C" {
                fn NSSearchPathForDirectoriesInDomains(
                    directory: usize,
                    domain: usize,
                    expand: Bool,
                ) -> *mut AnyObject;
            }
            let paths = NSSearchPathForDirectoriesInDomains(14, 1, Bool::YES);
            let count: usize = msg_send![paths, count];
            if count == 0 {
                bail!("Application Support unavailable");
            }
            let root: *mut AnyObject = msg_send![paths, objectAtIndex: 0usize];
            let unique = format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            );
            let directory = PathBuf::from(string(root)?)
                .join("gpui-clipboard-imports")
                .join(unique);
            std::fs::create_dir_all(directory.parent().unwrap())?;
            std::fs::create_dir(&directory)?;
            let destination = directory.join(&name);
            if let Err(error) = std::fs::copy(&source, &destination) {
                let _ = std::fs::remove_file(&destination);
                let _ = std::fs::remove_dir(&directory);
                return Err(error.into());
            }
            Ok(ImportedFile(destination))
        })();
        // A failed send drops the owned copy. A queued copy is likewise dropped
        // if its receiver is abandoned before polling it.
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(result);
        }
    });
    let progress: *mut AnyObject = msg_send![provider, loadFileRepresentationForTypeIdentifier: nsstring(uti), completionHandler: &*block];
    completion(rx, progress).await
}
pub(super) fn read() -> Result<Option<ClipboardItem>> {
    // The synchronous trait cannot await provider authorization or external I/O.
    // Poll once, without nested run-loop execution; cancellation is explicit.
    read_async()
        .now_or_never()
        .ok_or_else(|| anyhow!("Clipboard provider pending; use asynchronous clipboard read"))?
}
pub(super) async fn read_async() -> Result<Option<ClipboardItem>> {
    main_thread()?;
    unsafe {
        let pasteboard: *mut AnyObject = msg_send![class!(UIPasteboard), generalPasteboard];
        if pasteboard.is_null() {
            return Err(gpui::ClipboardReadError::Unavailable.into());
        }
        let providers: *mut AnyObject = msg_send![pasteboard, itemProviders];
        let Some(providers) = Retained::retain(providers) else {
            return Ok(None);
        };
        let count: usize = msg_send![&*providers, count];
        if count == 0 {
            return Ok(None);
        }
        let mut imported = Vec::new();
        let result: Result<ClipboardItem> = async {
            let mut entries = Vec::with_capacity(count);
            for index in 0..count {
                let provider: *mut AnyObject = msg_send![&*providers, objectAtIndex: index];
                let provider = &*provider;
                let file = has(provider, FILE);
                if !file {
                    if let Some((format, _, uti)) =
                        wire::FORMATS.iter().find(|(_, _, uti)| has(provider, uti))
                    {
                        entries.push(ClipboardEntry::Image(Image::from_bytes(
                            *format,
                            load(provider, uti).await?,
                        )));
                        continue;
                    }
                    if has(provider, TEXT) {
                        let text = String::from_utf8(load(provider, TEXT).await?)?;
                        let metadata = if has(provider, META) {
                            Some(String::from_utf8(load(provider, META).await?)?)
                        } else {
                            None
                        };
                        entries.push(ClipboardEntry::String(ClipboardString { text, metadata }));
                        continue;
                    }
                }
                let types: *mut AnyObject = msg_send![provider, registeredTypeIdentifiers];
                let length: usize = msg_send![types, count];
                let mut selected = None;
                for index in 0..length {
                    let uti: *mut AnyObject = msg_send![types, objectAtIndex: index];
                    let uti = string(uti)?;
                    if uti != FILE && uti != META {
                        selected = Some(uti);
                        break;
                    }
                }
                let uti = selected
                    .ok_or_else(|| anyhow!("Clipboard contains no supported representation"))?;
                let path = import_file(provider, &uti).await?;
                let copied_path = path.0.clone();
                imported.push(path);
                entries.push(ClipboardEntry::ExternalPaths(ExternalPaths(
                    std::iter::once(copied_path).collect(),
                )));
            }
            Ok(ClipboardItem { entries })
        }
        .await;
        if result.is_ok() {
            // Transfer successful imports to the caller; pending/error/drop
            // paths retain their RAII ownership and remove only their own files.
            for mut path in imported {
                let _ = std::mem::take(&mut path.0);
            }
        }
        result.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abandoning_progress_requests_native_cancellation() {
        objc2::rc::autoreleasepool(|_| unsafe {
            let native: *mut AnyObject =
                msg_send![class!(NSProgress), progressWithTotalUnitCount: 1i64];
            let native = Retained::retain(native).unwrap();
            let before: Bool = msg_send![&*native, isCancelled];
            assert!(!before.as_bool());
            let (sender, receiver) = oneshot::channel::<Result<Vec<u8>>>();
            // Poll the same completion future used by both native loaders.
            // Keeping sender alive makes this Pending rather than disconnected.
            let operation = completion(receiver, Retained::as_ptr(&native).cast_mut());
            assert!(operation.now_or_never().is_none());
            let after: Bool = msg_send![&*native, isCancelled];
            assert!(after.as_bool());
            assert!(sender.send(Ok(vec![1, 2, 3])).is_err());
        });
    }

    #[test]
    fn abandoned_file_delivery_removes_late_and_queued_copies() {
        for queued in [false, true] {
            let directory = std::env::temp_dir().join(format!(
                "gpui-clipboard-cancel-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                queued
            ));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("sentinel");
            std::fs::write(&path, b"owned fixture").unwrap();
            let (tx, rx) = oneshot::channel();
            if queued {
                assert!(tx.send(ImportedFile(path.clone())).is_ok());
                drop(rx);
            } else {
                drop(rx);
                drop(tx.send(ImportedFile(path.clone())));
            }
            assert!(!path.exists());
            assert!(!directory.exists());
        }
    }
    #[test]
    fn native_provider_retains_format_bytes_and_unicode_metadata() {
        objc2::rc::autoreleasepool(|_| unsafe {
            for (_, _, uti) in wire::FORMATS {
                let provider = provider().unwrap();
                register(&provider, uti, vec![0, 128, 255]);
                register(&provider, META, "a\0한😀".as_bytes().to_vec());
                assert_eq!(
                    futures::executor::block_on(load(&provider, uti)).unwrap(),
                    vec![0, 128, 255]
                );
                assert_eq!(
                    String::from_utf8(futures::executor::block_on(load(&provider, META)).unwrap())
                        .unwrap(),
                    "a\0한😀"
                );
            }
            let provider = provider().unwrap();
            register(&provider, TEXT, Vec::new());
            assert!(futures::executor::block_on(load(&provider, TEXT))
                .unwrap()
                .is_empty());
        });
    }
}
