use crate::ios::{cg_types::ObjcCGRect, platform_view::on_main, util::nsstring};
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};

static TEXT_KEY: u8 = 0;
static SUBJECT_KEY: u8 = 0;
extern "C" {
    fn objc_setAssociatedObject(
        object: *mut AnyObject,
        key: *const u8,
        value: *mut AnyObject,
        policy: usize,
    );
    fn objc_getAssociatedObject(object: *mut AnyObject, key: *const u8) -> *mut AnyObject;
}
unsafe extern "C" fn placeholder(
    this: *mut AnyObject,
    _: Sel,
    _: *mut AnyObject,
) -> *mut AnyObject {
    objc_getAssociatedObject(this, &TEXT_KEY)
}
unsafe extern "C" fn item(
    this: *mut AnyObject,
    _: Sel,
    _: *mut AnyObject,
    _: *mut AnyObject,
) -> *mut AnyObject {
    objc_getAssociatedObject(this, &TEXT_KEY)
}
unsafe extern "C" fn subject(
    this: *mut AnyObject,
    _: Sel,
    _: *mut AnyObject,
    _: *mut AnyObject,
) -> *mut AnyObject {
    objc_getAssociatedObject(this, &SUBJECT_KEY)
}
fn item_class() -> &'static AnyClass {
    static CLASS: std::sync::OnceLock<&'static AnyClass> = std::sync::OnceLock::new();
    CLASS.get_or_init(|| unsafe {
        let mut builder = ClassBuilder::new(c"GpuiShareItem", class!(NSObject)).unwrap();
        if let Some(protocol) = AnyProtocol::get(c"UIActivityItemSource") {
            builder.add_protocol(protocol);
        }
        builder.add_method(
            sel!(activityViewControllerPlaceholderItem:),
            placeholder
                as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> *mut AnyObject,
        );
        builder.add_method(
            sel!(activityViewController:itemForActivityType:),
            item as unsafe extern "C" fn(
                *mut AnyObject,
                Sel,
                *mut AnyObject,
                *mut AnyObject,
            ) -> *mut AnyObject,
        );
        builder.add_method(
            sel!(activityViewController:subjectForActivityType:),
            subject
                as unsafe extern "C" fn(
                    *mut AnyObject,
                    Sel,
                    *mut AnyObject,
                    *mut AnyObject,
                ) -> *mut AnyObject,
        );
        builder.register()
    })
}

pub fn share_text(text: &str, subject: Option<&str>) -> Result<(), String> {
    on_main(|| unsafe {
        let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        let window: *mut AnyObject = msg_send![app, keyWindow];
        if window.is_null() {
            return Err("No key window for share sheet".into());
        }
        let mut presenter: *mut AnyObject = msg_send![window, rootViewController];
        if presenter.is_null() {
            return Err("No share-sheet presenter".into());
        }
        loop {
            let next: *mut AnyObject = msg_send![presenter, presentedViewController];
            if next.is_null() {
                break;
            }
            presenter = next;
        }
        let dismissing: bool = msg_send![presenter, isBeingDismissed];
        if dismissing {
            return Err("Share presenter is being dismissed".into());
        }
        let source: *mut AnyObject = msg_send![item_class(), new];
        if source.is_null() {
            return Err("Could not allocate share item".into());
        }
        // Associated objects retain each request's strings for the sheet's lifetime.
        objc_setAssociatedObject(source, &TEXT_KEY, nsstring(text), 1);
        if let Some(subject) = subject {
            objc_setAssociatedObject(source, &SUBJECT_KEY, nsstring(subject), 1);
        }
        let items: *mut AnyObject = msg_send![class!(NSArray), arrayWithObject: source];
        let sheet: *mut AnyObject = msg_send![class!(UIActivityViewController), alloc];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let sheet: *mut AnyObject =
            msg_send![sheet, initWithActivityItems: items, applicationActivities: nil];
        let _: () = msg_send![source, release];
        if sheet.is_null() {
            return Err("Could not allocate share sheet".into());
        }
        let popover: *mut AnyObject = msg_send![sheet, popoverPresentationController];
        if !popover.is_null() {
            let view: *mut AnyObject = msg_send![presenter, view];
            let bounds: ObjcCGRect = msg_send![view, bounds];
            let _: () = msg_send![popover, setSourceView: view];
            let _: () = msg_send![popover, setSourceRect: ObjcCGRect::new(bounds.width / 2.0, bounds.height / 2.0, 1.0, 1.0)];
            let _: () = msg_send![popover, setPermittedArrowDirections: 0usize];
        }
        let _: () =
            msg_send![presenter, presentViewController: sheet, animated: true, completion: nil];
        let _: () = msg_send![sheet, release];
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activity_item_delivers_subject_and_text_independently() {
        on_main(|| unsafe {
            let source: *mut AnyObject = msg_send![item_class(), new];
            objc_setAssociatedObject(source, &TEXT_KEY, nsstring("body"), 1);
            objc_setAssociatedObject(source, &SUBJECT_KEY, nsstring("subject"), 1);
            let nil: *mut AnyObject = std::ptr::null_mut();
            let body: *mut AnyObject =
                msg_send![source, activityViewController: nil, itemForActivityType: nil];
            let title: *mut AnyObject =
                msg_send![source, activityViewController: nil, subjectForActivityType: nil];
            let body_matches: bool = msg_send![body, isEqualToString: nsstring("body")];
            let title_matches: bool = msg_send![title, isEqualToString: nsstring("subject")];
            let _: () = msg_send![source, release];
            assert!(body_matches && title_matches);
        });
    }
}
