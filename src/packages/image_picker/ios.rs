use super::{CameraDevice, ImagePickerOptions, ImageSource, PickedFile};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use std::sync::{mpsc, Once};

#[path = "../permission_handler/ios_completion.rs"]
mod completion;
#[path = "../file_selector/ios_delegate.rs"]
mod delegate;
#[path = "ios_image.rs"]
mod image;
#[path = "../file_selector/request.rs"]
mod request;

#[link(name = "PhotosUI", kind = "framework")]
extern "C" {}

#[link(name = "Photos", kind = "framework")]
extern "C" {}

#[link(name = "UIKit", kind = "framework")]
extern "C" {}

// ── Result channel ──────────────────────────────────────────────────────────

static REQUEST: request::Slot = request::Slot::new();

fn set_result_channel() -> Result<request::Request<'static>, String> {
    let main: Bool = unsafe { msg_send![class!(NSThread), isMainThread] };
    if !main.as_bool() {
        return Err("Call the iOS image picker on the main thread".into());
    }
    REQUEST.begin()
}

// ── PHPicker delegate (iOS 14+ gallery picker) ─────────────────────────────

static REGISTER_PHPICKER_DELEGATE: Once = Once::new();
static mut PHPICKER_DELEGATE_CLASS: *const AnyClass = std::ptr::null();

fn phpicker_delegate_class() -> &'static AnyClass {
    REGISTER_PHPICKER_DELEGATE.call_once(|| {
        let superclass = class!(NSObject);
        let mut decl = ClassBuilder::new(c"GpuiPHPickerDelegate", superclass).unwrap();

        unsafe {
            delegate::register(&mut decl);
            decl.add_method(
                sel!(picker:didFinishPicking:),
                phpicker_did_finish
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            );
        }

        unsafe { PHPICKER_DELEGATE_CLASS = decl.register() as *const AnyClass };
    });
    unsafe { &*PHPICKER_DELEGATE_CLASS }
}

extern "C" fn phpicker_did_finish(
    this: *mut AnyObject,
    _: Sel,
    picker: *mut AnyObject,
    results: *mut AnyObject,
) {
    unsafe {
        let Some(token) = delegate::token(this) else {
            return;
        };
        if !token.claim() {
            return;
        }
        let result = (|| -> Result<Vec<String>, String> {
            let count: usize = msg_send![results, count];
            let mut imported = request::Imports::new();
            for index in 0..count {
                let result: *mut AnyObject = msg_send![results, objectAtIndex: index];
                let provider: *mut AnyObject = msg_send![result, itemProvider];
                let movie: Bool = msg_send![provider, hasItemConformingToTypeIdentifier: nsstring("public.movie")];
                let uti = nsstring(if movie.as_bool() {
                    "public.movie"
                } else {
                    "public.image"
                });
                let supported: Bool = msg_send![provider, hasItemConformingToTypeIdentifier: uti];
                if !supported.as_bool() {
                    return Err(
                        "Selected item has no supported image or video representation".into(),
                    );
                }
                let (sender, receiver) = mpsc::channel::<Result<request::Imports, String>>();
                let sender = std::sync::Mutex::new(Some(sender));
                let block =
                    block2::RcBlock::new(move |url: *mut AnyObject, error: *mut AnyObject| {
                        // Provider completions are independently one-shot, before any file creation.
                        let Some(sender) = sender.lock().ok().and_then(|mut sender| sender.take())
                        else {
                            return;
                        };
                        let result = (|| -> Result<request::Imports, String> {
                            if !error.is_null() {
                                return Err(completion::error_message(error));
                            }
                            if url.is_null() {
                                return Err("Photo provider returned no file URL".into());
                            }
                            let value: *mut AnyObject = msg_send![url, path];
                            if value.is_null() {
                                return Err("Photo provider returned no path".into());
                            }
                            let bytes: *const std::ffi::c_char = msg_send![value, UTF8String];
                            if bytes.is_null() {
                                return Err("Photo provider returned an unreadable path".into());
                            }
                            let source = std::ffi::CStr::from_ptr(bytes).to_string_lossy();
                            // Copy before returning: the provider owns the temporary URL lifetime.
                            let path =
                                completion::import_file(std::path::Path::new(source.as_ref()))?;
                            let mut imported = request::Imports::new();
                            imported.push(path);
                            Ok(imported)
                        })();
                        // Both a failed send and an unread queued value retain RAII cleanup.
                        let _ = sender.send(result);
                    });
                let progress: *mut AnyObject = msg_send![provider, loadFileRepresentationForTypeIdentifier: uti, completionHandler: &*block];
                let _: () = msg_send![progress, retain];
                let result = completion::wait_cancellable(&receiver, || token.is_open());
                if result.is_err() {
                    let _: () = msg_send![progress, cancel];
                }
                let _: () = msg_send![progress, release];
                for path in result??.take() {
                    imported.push(path);
                }
            }
            Ok(imported.take())
        })();
        delegate::dismiss(picker, token, result);
    }
}

// ── UIImagePicker delegate (camera capture) ─────────────────────────────────

static REGISTER_UIPICKER_DELEGATE: Once = Once::new();
static mut UIPICKER_DELEGATE_CLASS: *const AnyClass = std::ptr::null();

fn uipicker_delegate_class() -> &'static AnyClass {
    REGISTER_UIPICKER_DELEGATE.call_once(|| {
        let superclass = class!(NSObject);
        let mut decl = ClassBuilder::new(c"GpuiUIImagePickerDelegate", superclass).unwrap();

        unsafe {
            delegate::register(&mut decl);
            decl.add_method(
                sel!(imagePickerController:didFinishPickingMediaWithInfo:),
                uipicker_did_finish
                    as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            );
            decl.add_method(
                sel!(imagePickerControllerDidCancel:),
                uipicker_did_cancel as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
        }

        unsafe { UIPICKER_DELEGATE_CLASS = decl.register() as *const AnyClass };
    });
    unsafe { &*UIPICKER_DELEGATE_CLASS }
}

unsafe extern "C" fn uipicker_did_finish(
    this: *mut AnyObject,
    _: Sel,
    controller: *mut AnyObject,
    info: *mut AnyObject,
) {
    let Some(token) = delegate::token(this) else {
        return;
    };
    if !token.claim() {
        return;
    }
    let result = (|| -> Result<Vec<String>, String> {
        let media_url: *mut AnyObject =
            msg_send![info, objectForKey: nsstring("UIImagePickerControllerMediaURL")];
        if !media_url.is_null() {
            let value: *mut AnyObject = msg_send![media_url, path];
            let bytes: *const std::ffi::c_char = msg_send![value, UTF8String];
            if bytes.is_null() {
                return Err("Camera returned an unreadable media path".into());
            }
            let path = std::ffi::CStr::from_ptr(bytes).to_string_lossy();
            return completion::import_file(std::path::Path::new(path.as_ref()))
                .map(|path| vec![path]);
        }
        let image: *mut AnyObject =
            msg_send![info, objectForKey: nsstring("UIImagePickerControllerOriginalImage")];
        if image.is_null() {
            return Err("Camera returned no readable media".into());
        }
        let data = uiimage_jpeg_representation(image, 1.0);
        if data.is_null() {
            return Err("Cannot encode camera image".into());
        }
        let directory = completion::private_import_directory()?;
        let path = directory
            .join("captured.jpg")
            .to_string_lossy()
            .into_owned();
        let mut imported = request::Imports::new();
        imported.push(path.clone()); // RAII reclaims partial file and directory on every error.
        let wrote: Bool = msg_send![data, writeToFile: nsstring(&path), atomically: true];
        if !wrote.as_bool() {
            return Err("Cannot save camera image".into());
        }
        Ok(imported.take())
    })();
    delegate::dismiss(controller, token, result);
}

unsafe extern "C" fn uipicker_did_cancel(this: *mut AnyObject, _: Sel, controller: *mut AnyObject) {
    let Some(token) = delegate::token(this) else {
        return;
    };
    if token.claim() {
        delegate::dismiss(controller, token, Ok(vec![]));
    }
}

// UIImageJPEGRepresentation is a C function, not an ObjC method
extern "C" {
    fn UIImageJPEGRepresentation(image: *mut AnyObject, quality: f64) -> *mut AnyObject;
}

unsafe fn uiimage_jpeg_representation(image: *mut AnyObject, quality: f64) -> *mut AnyObject {
    UIImageJPEGRepresentation(image, quality)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

use crate::ios::util::nsstring;

unsafe fn present_vc(vc: *mut AnyObject) -> Result<(), String> {
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
        presentViewController: vc,
        animated: true,
        completion: std::ptr::null::<AnyObject>()
    ];
    Ok(())
}

fn path_to_picked_file(path: &str) -> PickedFile {
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    PickedFile {
        path: path.to_string(),
        name,
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

pub fn pick_image(options: &ImagePickerOptions) -> Result<Option<PickedFile>, String> {
    match options.source {
        ImageSource::Gallery => {
            pick_image_from_gallery(options, false).map(|v| v.into_iter().next())
        }
        ImageSource::Camera => pick_from_camera(options, false),
    }
}

pub fn pick_multi_image(
    _max_width: Option<f64>,
    _max_height: Option<f64>,
    _image_quality: Option<u8>,
) -> Result<Vec<PickedFile>, String> {
    let options = ImagePickerOptions {
        source: ImageSource::Gallery,
        max_width: _max_width,
        max_height: _max_height,
        image_quality: _image_quality,
        preferred_camera: CameraDevice::Rear,
    };
    pick_image_from_gallery(&options, true)
}

pub fn pick_video(
    source: ImageSource,
    preferred_camera: CameraDevice,
) -> Result<Option<PickedFile>, String> {
    let options = ImagePickerOptions {
        source,
        max_width: None,
        max_height: None,
        image_quality: None,
        preferred_camera,
    };
    match source {
        ImageSource::Gallery => pick_video_from_gallery(),
        ImageSource::Camera => pick_from_camera(&options, true),
    }
}

fn pick_image_from_gallery(
    options: &ImagePickerOptions,
    allow_multi: bool,
) -> Result<Vec<PickedFile>, String> {
    let rx = set_result_channel()?;

    unsafe {
        // PHPickerConfiguration
        let config: *mut AnyObject = msg_send![class!(PHPickerConfiguration), alloc];
        let config: *mut AnyObject = msg_send![config, init];

        if allow_multi {
            let _: () = msg_send![config, setSelectionLimit: 0i64]; // 0 = unlimited
        } else {
            let _: () = msg_send![config, setSelectionLimit: 1i64];
        }

        // Filter to images only
        let image_filter: *mut AnyObject = msg_send![class!(PHPickerFilter), imagesFilter];
        let _: () = msg_send![config, setFilter: image_filter];

        // Create PHPickerViewController
        let picker: *mut AnyObject = msg_send![class!(PHPickerViewController), alloc];
        let picker: *mut AnyObject = msg_send![picker, initWithConfiguration: config];
        let _: () = msg_send![config, release];
        if picker.is_null() {
            return Err("Failed to create PHPickerViewController".into());
        }

        // Set delegate
        let delegate_cls = phpicker_delegate_class();
        let delegate: *mut AnyObject = msg_send![delegate_cls, alloc];
        let delegate: *mut AnyObject = msg_send![delegate, init];
        delegate::bind(picker, delegate, rx.token.clone())?;

        let presented = present_vc(picker);
        let _: () = msg_send![picker, release];
        presented?;
    }

    match completion::wait(&rx.receiver)? {
        Ok(paths) if !allow_multi && paths.len() > 1 => {
            request::discard(&paths);
            Err("Single image picker returned multiple files".into())
        }
        Ok(paths) => Ok(image::process(paths, options)?
            .iter()
            .map(|p| path_to_picked_file(p))
            .collect()),
        Err(error) => Err(error),
    }
}

fn pick_video_from_gallery() -> Result<Option<PickedFile>, String> {
    let rx = set_result_channel()?;

    unsafe {
        // PHPickerConfiguration
        let config: *mut AnyObject = msg_send![class!(PHPickerConfiguration), alloc];
        let config: *mut AnyObject = msg_send![config, init];
        let _: () = msg_send![config, setSelectionLimit: 1i64];

        // Filter to videos only
        let video_filter: *mut AnyObject = msg_send![class!(PHPickerFilter), videosFilter];
        let _: () = msg_send![config, setFilter: video_filter];

        let picker: *mut AnyObject = msg_send![class!(PHPickerViewController), alloc];
        let picker: *mut AnyObject = msg_send![picker, initWithConfiguration: config];
        let _: () = msg_send![config, release];
        if picker.is_null() {
            return Err("Failed to create PHPickerViewController".into());
        }

        let delegate_cls = phpicker_delegate_class();
        let delegate: *mut AnyObject = msg_send![delegate_cls, alloc];
        let delegate: *mut AnyObject = msg_send![delegate, init];
        delegate::bind(picker, delegate, rx.token.clone())?;

        let presented = present_vc(picker);
        let _: () = msg_send![picker, release];
        presented?;
    }

    match completion::wait(&rx.receiver)? {
        Ok(paths) if paths.is_empty() => Ok(None),
        Ok(paths) if paths.len() != 1 => {
            request::discard(&paths);
            Err("Single video picker returned multiple files".into())
        }
        Ok(paths) => Ok(Some(path_to_picked_file(&paths[0]))),
        Err(error) => Err(error),
    }
}

fn pick_from_camera(
    options: &ImagePickerOptions,
    video: bool,
) -> Result<Option<PickedFile>, String> {
    let rx = set_result_channel()?;

    unsafe {
        // Check if camera is available
        let source_type: i64 = 1; // UIImagePickerControllerSourceTypeCamera
        let available: Bool =
            msg_send![class!(UIImagePickerController), isSourceTypeAvailable: source_type];
        if !available.as_bool() {
            return Err("Camera is not available on this device".into());
        }

        let picker: *mut AnyObject = msg_send![class!(UIImagePickerController), alloc];
        let picker: *mut AnyObject = msg_send![picker, init];
        if picker.is_null() {
            return Err("Failed to create UIImagePickerController".into());
        }

        let _: () = msg_send![picker, setSourceType: source_type];

        // Set media types
        if video {
            let video_type = nsstring("public.movie");
            let types: *mut AnyObject = msg_send![class!(NSArray), arrayWithObject: video_type];
            let _: () = msg_send![picker, setMediaTypes: types];
        }

        // Set camera device
        let camera_device: i64 = match options.preferred_camera {
            CameraDevice::Rear => 0,  // UIImagePickerControllerCameraDeviceRear
            CameraDevice::Front => 1, // UIImagePickerControllerCameraDeviceFront
        };
        let _: () = msg_send![picker, setCameraDevice: camera_device];

        // Set delegate
        let delegate_cls = uipicker_delegate_class();
        let delegate: *mut AnyObject = msg_send![delegate_cls, alloc];
        let delegate: *mut AnyObject = msg_send![delegate, init];
        delegate::bind(picker, delegate, rx.token.clone())?;

        let presented = present_vc(picker);
        let _: () = msg_send![picker, release];
        presented?;
    }

    match completion::wait(&rx.receiver)? {
        Ok(paths) if paths.is_empty() => Ok(None),
        Ok(paths) if paths.len() != 1 => {
            request::discard(&paths);
            Err("Camera returned multiple files".into())
        }
        Ok(paths) => {
            let paths = if video {
                paths
            } else {
                image::process(paths, options)?
            };
            Ok(paths.first().map(|path| path_to_picked_file(path)))
        }
        Err(error) => Err(error),
    }
}
