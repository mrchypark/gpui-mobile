//! Per-controller token; no callback ever targets a global/current sender.
use super::{completion, request};
use objc2::runtime::{AnyObject, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use std::ffi::c_void;

unsafe fn token_pointer(delegate: *mut AnyObject) -> *mut *mut c_void {
    let object = &*delegate;
    object
        .class()
        .instance_variable(c"gpui_request")
        .unwrap()
        .load_ptr(object)
}
pub unsafe fn token(delegate: *mut AnyObject) -> Option<request::Token> {
    let ptr = *token_pointer(delegate) as *mut request::Token;
    ptr.as_ref().cloned()
}
pub unsafe fn bind(
    picker: *mut AnyObject,
    delegate: *mut AnyObject,
    token: request::Token,
) -> Result<(), String> {
    if delegate.is_null() {
        let _: () = msg_send![picker, release];
        return Err("Cannot allocate picker delegate".into());
    }
    *token_pointer(delegate) = Box::into_raw(Box::new(token)).cast();
    completion::set_picker_delegate(picker, delegate)?;
    let presentation: *mut AnyObject = msg_send![picker, presentationController];
    let _: () = msg_send![presentation, setDelegate: delegate];
    Ok(())
}
pub unsafe fn register(builder: &mut ClassBuilder) {
    builder.add_ivar::<*mut c_void>(c"gpui_request");
    builder.add_method(
        sel!(dealloc),
        dealloc as unsafe extern "C" fn(*mut AnyObject, Sel),
    );
    builder.add_method(
        sel!(presentationControllerDidDismiss:),
        did_dismiss as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
    );
}
unsafe extern "C" fn did_dismiss(this: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    if let Some(token) = token(this) {
        if token.claim() {
            token.finish(Ok(vec![]));
        } else if !token.is_dismissing() {
            token.finish(Err("Picker dismissed while importing".into()));
        }
    }
}
unsafe extern "C" fn dealloc(this: *mut AnyObject, _: Sel) {
    let ptr = *token_pointer(this) as *mut request::Token;
    if !ptr.is_null() {
        *token_pointer(this) = std::ptr::null_mut();
        let token = Box::from_raw(ptr);
        if !token.is_dismissing() {
            token.finish(Err("Picker controller closed without a result".into()));
        }
    }
    let _: () = msg_send![super(this, class!(NSObject)), dealloc];
}

pub unsafe fn dismiss(
    picker: *mut AnyObject,
    token: request::Token,
    result: request::PickerResult,
) {
    if !token.begin_dismissal() {
        token.finish(result);
        return;
    }
    // Retain the busy guard until dismissal completes. This block can be copied by UIKit.
    struct Delivery {
        token: request::Token,
        result: Option<request::PickerResult>,
    }
    impl Drop for Delivery {
        fn drop(&mut self) {
            if let Some(result) = self.result.take() {
                if let Ok(paths) = result {
                    request::discard(&paths);
                }
                self.token
                    .finish(Err("Picker dismissal ended without completion".into()));
            }
        }
    }
    let delivery = std::sync::Mutex::new(Delivery {
        token,
        result: Some(result),
    });
    let done = block2::RcBlock::new(move || {
        if let Ok(mut delivery) = delivery.lock() {
            if let Some(result) = delivery.result.take() {
                delivery.token.finish(result);
            }
        }
    });
    let _: () = msg_send![picker, dismissViewControllerAnimated: true, completion: &*done];
}
