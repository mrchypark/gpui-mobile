//! A retained Quick Look data source; UIKit owns the presented controller lifetime.
use super::super::util::nsstring;
use anyhow::{Result, anyhow, bail};
use objc2::{
    class, msg_send,
    runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel},
    sel,
};
use std::{path::Path, sync::OnceLock};
#[link(name = "QuickLook", kind = "framework")]
unsafe extern "C" {}
unsafe extern "C" {
    fn objc_setAssociatedObject(
        object: *mut AnyObject,
        key: *const u8,
        value: *mut AnyObject,
        policy: usize,
    );
    fn objc_getAssociatedObject(object: *mut AnyObject, key: *const u8) -> *mut AnyObject;
}
static URL: u8 = 0;
static SOURCE: u8 = 0;
unsafe extern "C" fn count(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> isize {
    1
}
unsafe extern "C" fn item(
    this: *mut AnyObject,
    _: Sel,
    _: *mut AnyObject,
    index: isize,
) -> *mut AnyObject {
    if index != 0 {
        return std::ptr::null_mut();
    }
    objc_getAssociatedObject(this, &URL)
}
fn source_class() -> &'static AnyClass {
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut builder = ClassBuilder::new(c"GpuiPlatformPreviewSource", class!(NSObject))
            .expect("unique preview source class");
        unsafe {
            builder.add_method(
                sel!(numberOfPreviewItemsInPreviewController:),
                count as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> isize,
            );
            builder.add_method(
                sel!(previewController:previewItemAtIndex:),
                item as unsafe extern "C" fn(
                    *mut AnyObject,
                    Sel,
                    *mut AnyObject,
                    isize,
                ) -> *mut AnyObject,
            );
        }
        builder.register()
    })
}
pub(super) fn open(path: &Path) -> Result<()> {
    let main: Bool = unsafe { msg_send![class!(NSThread), isMainThread] };
    if !main.as_bool() {
        bail!("Quick Look presentation requires UIKit main thread");
    }
    let path = path.canonicalize()?;
    if !path.is_file() {
        bail!("Quick Look requires a regular file");
    }
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("Preview path is not UTF-8"))?;
    objc2::rc::autoreleasepool(|_| unsafe {
        let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        let window: *mut AnyObject = msg_send![app, keyWindow];
        let root: *mut AnyObject = msg_send![window, rootViewController];
        if root.is_null() {
            bail!("No active preview presenter");
        }
        let presented: *mut AnyObject = msg_send![root, presentedViewController];
        if !presented.is_null() {
            bail!("Another controller is already presented");
        }
        let url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: nsstring(path)];
        let supported: Bool = msg_send![class!(QLPreviewController), canPreviewItem: url];
        if !supported.as_bool() {
            bail!("Quick Look cannot preview this file type");
        }
        let controller: *mut AnyObject = msg_send![class!(QLPreviewController), new];
        let controller = objc2::rc::Retained::<AnyObject>::from_raw(controller)
            .ok_or_else(|| anyhow!("Cannot create Quick Look controller"))?;
        let source: *mut AnyObject = msg_send![source_class(), new];
        let source = objc2::rc::Retained::<AnyObject>::from_raw(source)
            .ok_or_else(|| anyhow!("Cannot create Quick Look data source"))?;
        objc_setAssociatedObject((&*source as *const AnyObject).cast_mut(), &URL, url, 1);
        objc_setAssociatedObject(
            (&*controller as *const AnyObject).cast_mut(),
            &SOURCE,
            (&*source as *const AnyObject).cast_mut(),
            1,
        );
        let _: () = msg_send![&*controller, setDataSource: &*source];
        let _: () = msg_send![root, presentViewController: &*controller, animated: Bool::YES, completion: std::ptr::null::<AnyObject>()];
        Ok(())
    })
}
