use super::{OpenFileOptions, SaveFileOptions, SelectedFile, TypeGroup};
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use std::sync::Once;

#[path = "../permission_handler/ios_completion.rs"]
mod completion;
#[path = "ios_delegate.rs"]
mod delegate;
#[path = "request.rs"]
mod request;

#[link(name = "UIKit", kind = "framework")]
extern "C" {}

#[link(name = "UniformTypeIdentifiers", kind = "framework")]
extern "C" {}

// ── Result channel ──────────────────────────────────────────────────────────

static REQUEST: request::Slot = request::Slot::new();

fn set_result_channel() -> Result<request::Request<'static>, String> {
    let main: objc2::runtime::Bool = unsafe { msg_send![class!(NSThread), isMainThread] };
    if !main.as_bool() {
        return Err("Call the iOS document picker on the main thread".into());
    }
    REQUEST.begin()
}

// ── ObjC delegate class ─────────────────────────────────────────────────────

static REGISTER_DELEGATE: Once = Once::new();
static mut DELEGATE_CLASS: *const AnyClass = std::ptr::null();

fn delegate_class() -> &'static AnyClass {
    REGISTER_DELEGATE.call_once(|| {
        let superclass = class!(NSObject);
        let mut decl = ClassBuilder::new(c"GpuiDocumentPickerDelegate", superclass).unwrap();

        unsafe {
            delegate::register(&mut decl);
            // documentPicker:didPickDocumentsAtURLs:
            decl.add_method(
                sel!(documentPicker:didPickDocumentsAtURLs:),
                did_pick_documents
                    as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            );
            // documentPickerWasCancelled:
            decl.add_method(
                sel!(documentPickerWasCancelled:),
                did_cancel as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
        }

        unsafe { DELEGATE_CLASS = decl.register() as *const AnyClass };
    });
    unsafe { &*DELEGATE_CLASS }
}

unsafe extern "C" fn did_pick_documents(
    this: *mut AnyObject,
    _: Sel,
    controller: *mut AnyObject,
    urls: *mut AnyObject,
) {
    let Some(token) = delegate::token(this) else {
        return;
    };
    if !token.claim() {
        return;
    }
    let result = (|| -> Result<Vec<String>, String> {
        let count: usize = msg_send![urls, count];
        let mut imported = request::Imports::new();
        if count == 0 {
            return Err("Document picker returned no file URL".into());
        }
        for index in 0..count {
            let url: *mut AnyObject = msg_send![urls, objectAtIndex: index];
            let value: *mut AnyObject = msg_send![url, path];
            if value.is_null() {
                return Err("Document picker returned an unreadable file URL".into());
            }
            let bytes: *const std::ffi::c_char = msg_send![value, UTF8String];
            if bytes.is_null() {
                return Err("Document picker returned an unreadable file path".into());
            }
            let path = std::ffi::CStr::from_ptr(bytes).to_string_lossy();
            imported.push(completion::import_file(std::path::Path::new(
                path.as_ref(),
            ))?);
        }
        Ok(imported.take())
    })();
    delegate::dismiss(controller, token, result);
}

unsafe extern "C" fn did_cancel(this: *mut AnyObject, _: Sel, controller: *mut AnyObject) {
    let Some(token) = delegate::token(this) else {
        return;
    };
    if token.claim() {
        delegate::dismiss(controller, token, Ok(vec![]));
    }
}

// ── UTType helpers ──────────────────────────────────────────────────────────

/// Convert a TypeGroup into an NSArray of UTType objects.
unsafe fn type_group_to_uttypes(group: &TypeGroup) -> *mut AnyObject {
    let mut types: Vec<*mut AnyObject> = Vec::new();

    // Prefer explicit UTIs
    for uti in &group.utis {
        let ut = uttype_from_identifier(uti);
        if !ut.is_null() {
            types.push(ut);
        }
    }

    // Fall back to extensions
    if types.is_empty() {
        for ext in &group.extensions {
            let ut = uttype_from_extension(ext);
            if !ut.is_null() {
                types.push(ut);
            }
        }
    }

    // Fall back to MIME types
    if types.is_empty() {
        for mime in &group.mime_types {
            let ut = uttype_from_mime(mime);
            if !ut.is_null() {
                types.push(ut);
            }
        }
    }

    if types.is_empty() {
        // Accept all file types
        let ut = uttype_from_identifier("public.item");
        if !ut.is_null() {
            types.push(ut);
        }
    }

    nsarray_from_objects(&types)
}

unsafe fn uttype_from_identifier(ident: &str) -> *mut AnyObject {
    let ns_ident = nsstring(ident);
    msg_send![class!(UTType), typeWithIdentifier: ns_ident]
}

unsafe fn uttype_from_extension(ext: &str) -> *mut AnyObject {
    let ns_ext = nsstring(ext);
    msg_send![class!(UTType), typeWithFilenameExtension: ns_ext]
}

unsafe fn uttype_from_mime(mime: &str) -> *mut AnyObject {
    let ns_mime = nsstring(mime);
    msg_send![class!(UTType), typeWithMIMEType: ns_mime]
}

use crate::ios::util::nsstring;

unsafe fn nsarray_from_objects(objects: &[*mut AnyObject]) -> *mut AnyObject {
    msg_send![class!(NSArray),
        arrayWithObjects: objects.as_ptr(),
        count: objects.len()
    ]
}

// ── Presentation helper ─────────────────────────────────────────────────────

unsafe fn present_picker(picker: *mut AnyObject) -> Result<(), String> {
    let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
    let key_window: *mut AnyObject = msg_send![app, keyWindow];
    if key_window.is_null() {
        return Err("No key window available".into());
    }
    let root_vc: *mut AnyObject = msg_send![key_window, rootViewController];
    if root_vc.is_null() {
        return Err("No root view controller".into());
    }
    let presented: *mut AnyObject = msg_send![root_vc, presentedViewController];
    if !presented.is_null() {
        return Err("Another view controller is already presented".into());
    }
    let _: () = msg_send![root_vc,
        presentViewController: picker,
        animated: true,
        completion: std::ptr::null::<AnyObject>()
    ];
    Ok(())
}

fn url_to_selected_file(url_string: &str) -> SelectedFile {
    // Extract filename from URL path
    let name = url_string
        .rsplit('/')
        .next()
        .unwrap_or(url_string)
        .to_string();
    // NSURL.path has already decoded the file URL in the native callback.
    let path = url_string.to_string();
    SelectedFile { path, name }
}

// ── Public API ──────────────────────────────────────────────────────────────

pub fn open_file(options: &OpenFileOptions) -> Result<Option<SelectedFile>, String> {
    let rx = set_result_channel()?;
    unsafe {
        let content_types = if options.accept_type_groups.is_empty() {
            let ut = uttype_from_identifier("public.item");
            nsarray_from_objects(&[ut])
        } else {
            // Merge all type groups into one array
            let mut all_types: Vec<*mut AnyObject> = Vec::new();
            for group in &options.accept_type_groups {
                let arr = type_group_to_uttypes(group);
                let count: usize = msg_send![arr, count];
                for i in 0..count {
                    let obj: *mut AnyObject = msg_send![arr, objectAtIndex: i];
                    all_types.push(obj);
                }
            }
            nsarray_from_objects(&all_types)
        };

        // UIDocumentPickerViewController *picker =
        //   [[UIDocumentPickerViewController alloc] initForOpeningContentTypes:types];
        let picker: *mut AnyObject = msg_send![class!(UIDocumentPickerViewController), alloc];
        let picker: *mut AnyObject =
            msg_send![picker, initForOpeningContentTypes: content_types, asCopy: true];
        if picker.is_null() {
            return Err("Failed to create UIDocumentPickerViewController".into());
        }

        let _: () =
            msg_send![picker, setAllowsMultipleSelection: objc2::runtime::Bool::from(false)];

        // Set delegate
        let delegate_cls = delegate_class();
        let delegate: *mut AnyObject = msg_send![delegate_cls, alloc];
        let delegate: *mut AnyObject = msg_send![delegate, init];
        delegate::bind(picker, delegate, rx.token.clone())?;

        let presented = present_picker(picker);
        let _: () = msg_send![picker, release];
        presented?;
    }

    // Wait for result (blocks until user picks or cancels)
    match completion::wait(&rx.receiver)? {
        Ok(paths) if paths.is_empty() => Ok(None),
        Ok(paths) if paths.len() != 1 => {
            request::discard(&paths);
            Err("Single document picker returned multiple files".into())
        }
        Ok(paths) => Ok(Some(url_to_selected_file(&paths[0]))),
        Err(error) => Err(error),
    }
}

pub fn open_files(options: &OpenFileOptions) -> Result<Vec<SelectedFile>, String> {
    let rx = set_result_channel()?;
    unsafe {
        let content_types = if options.accept_type_groups.is_empty() {
            let ut = uttype_from_identifier("public.item");
            nsarray_from_objects(&[ut])
        } else {
            let mut all_types: Vec<*mut AnyObject> = Vec::new();
            for group in &options.accept_type_groups {
                let arr = type_group_to_uttypes(group);
                let count: usize = msg_send![arr, count];
                for i in 0..count {
                    let obj: *mut AnyObject = msg_send![arr, objectAtIndex: i];
                    all_types.push(obj);
                }
            }
            nsarray_from_objects(&all_types)
        };

        let picker: *mut AnyObject = msg_send![class!(UIDocumentPickerViewController), alloc];
        let picker: *mut AnyObject =
            msg_send![picker, initForOpeningContentTypes: content_types, asCopy: true];
        if picker.is_null() {
            return Err("Failed to create UIDocumentPickerViewController".into());
        }

        let _: () = msg_send![picker, setAllowsMultipleSelection: true];

        let delegate_cls = delegate_class();
        let delegate: *mut AnyObject = msg_send![delegate_cls, alloc];
        let delegate: *mut AnyObject = msg_send![delegate, init];
        delegate::bind(picker, delegate, rx.token.clone())?;

        let presented = present_picker(picker);
        let _: () = msg_send![picker, release];
        presented?;
    }

    match completion::wait(&rx.receiver)? {
        Ok(paths) => Ok(paths.iter().map(|p| url_to_selected_file(p)).collect()),
        Err(error) => Err(error),
    }
}

pub fn get_save_path(_options: &SaveFileOptions) -> Result<Option<String>, String> {
    Err("iOS cannot return a writable save path without exporting file contents; this path-only API is unsupported".into())
}
pub fn get_directory_path(_initial_directory: Option<&str>) -> Result<Option<String>, String> {
    Err("iOS directory access requires a security-scoped lifetime which this path-only API cannot represent".into())
}
