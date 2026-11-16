//! Concrete UITextInputTraits accessors for the GPUI insertion proxy.
use objc2::runtime::{AnyObject, Bool, ClassBuilder, Sel};
use objc2::{msg_send, sel};

pub(super) fn register(decl: &mut ClassBuilder) {
    // UIKit may forward these protocol properties rather than expose methods
    // discoverable by objc2's checked sends. Own their storage on our subclass.
    macro_rules! property {
        ($ivar:literal, $name:literal, $getter:ident, $setter:ident) => {{
            decl.add_ivar::<isize>($ivar);
            #[allow(deprecated)]
            unsafe extern "C" fn get(this: *mut AnyObject, _: Sel) -> isize {
                unsafe { *(*this).get_ivar::<isize>($name) }
            }
            #[allow(deprecated)]
            unsafe extern "C" fn set(this: *mut AnyObject, _: Sel, value: isize) {
                unsafe { *(*this).get_mut_ivar::<isize>($name) = value; }
            }
            unsafe {
                decl.add_method(sel!($getter), get as unsafe extern "C" fn(*mut AnyObject, Sel) -> isize);
                decl.add_method(sel!($setter:), set as unsafe extern "C" fn(*mut AnyObject, Sel, isize));
            }
        }};
    }
    property!(
        c"gpui_keyboard_type",
        "gpui_keyboard_type",
        keyboardType,
        setKeyboardType
    );
    property!(
        c"gpui_autocorrection",
        "gpui_autocorrection",
        autocorrectionType,
        setAutocorrectionType
    );
    property!(
        c"gpui_capitalization",
        "gpui_capitalization",
        autocapitalizationType,
        setAutocapitalizationType
    );
    property!(
        c"gpui_spellchecking",
        "gpui_spellchecking",
        spellCheckingType,
        setSpellCheckingType
    );
    property!(
        c"gpui_return_key",
        "gpui_return_key",
        returnKeyType,
        setReturnKeyType
    );

    // UITextView does not implement the UITextField secure-text property. Own
    // both selectors so keyboard configuration updates and live proxy checks
    // use the same BOOL storage.
    decl.add_ivar::<Bool>(c"gpui_secure_text_entry");
    #[allow(deprecated)]
    unsafe extern "C" fn secure_text_entry(this: *mut AnyObject, _: Sel) -> Bool {
        unsafe { *(*this).get_ivar::<Bool>("gpui_secure_text_entry") }
    }
    #[allow(deprecated)]
    unsafe extern "C" fn set_secure_text_entry(this: *mut AnyObject, _: Sel, value: Bool) {
        unsafe { *(*this).get_mut_ivar::<Bool>("gpui_secure_text_entry") = value };
    }
    unsafe {
        decl.add_method(
            sel!(isSecureTextEntry),
            secure_text_entry as unsafe extern "C" fn(*mut AnyObject, Sel) -> Bool,
        );
        decl.add_method(
            sel!(setSecureTextEntry:),
            set_secure_text_entry as unsafe extern "C" fn(*mut AnyObject, Sel, Bool),
        );
    }
}

/// Initialize after UITextView init; the text traits use integer ABI and secure entry uses BOOL.
pub(super) unsafe fn initialize(view: *mut AnyObject) {
    unsafe {
        let _: () = msg_send![view, setKeyboardType: 0_isize];
        let _: () = msg_send![view, setSecureTextEntry: Bool::NO];
        let _: () = msg_send![view, setAutocorrectionType: 0_isize];
        let _: () = msg_send![view, setAutocapitalizationType: 2_isize];
        let _: () = msg_send![view, setSpellCheckingType: 0_isize];
        let _: () = msg_send![view, setReturnKeyType: 0_isize];
    }
}

/// Run on the actual allocated production proxy, not a stand-in UITextView.
#[cfg(debug_assertions)]
pub(super) unsafe fn verify_live_proxy(view: *mut AnyObject) {
    macro_rules! check {
        ($get:ident, $set:ident, $default:expr, $alternate:expr) => {{
            let original: isize = unsafe { msg_send![view, $get] };
            assert_eq!(original, $default, stringify!($get));
            let _: () = unsafe { msg_send![view, $set: $alternate] };
            let actual: isize = unsafe { msg_send![view, $get] };
            assert_eq!(actual, $alternate, stringify!($set));
            let _: () = unsafe { msg_send![view, $set: original] };
            let restored: isize = unsafe { msg_send![view, $get] };
            assert_eq!(restored, original, stringify!($get));
        }};
    }
    check!(keyboardType, setKeyboardType, 0_isize, 7_isize);
    let has_secure_getter: Bool =
        unsafe { msg_send![view, respondsToSelector: sel!(isSecureTextEntry)] };
    assert!(
        has_secure_getter.as_bool(),
        "GPUITextInputView is missing isSecureTextEntry"
    );
    let has_secure_setter: Bool =
        unsafe { msg_send![view, respondsToSelector: sel!(setSecureTextEntry:)] };
    assert!(
        has_secure_setter.as_bool(),
        "GPUITextInputView is missing setSecureTextEntry:"
    );
    let secure: Bool = unsafe { msg_send![view, isSecureTextEntry] };
    assert!(!secure.as_bool(), "isSecureTextEntry");
    let _: () = unsafe { msg_send![view, setSecureTextEntry: Bool::YES] };
    let secure: Bool = unsafe { msg_send![view, isSecureTextEntry] };
    assert!(secure.as_bool(), "setSecureTextEntry");
    let _: () = unsafe { msg_send![view, setSecureTextEntry: Bool::NO] };
    check!(autocorrectionType, setAutocorrectionType, 0_isize, 1_isize);
    check!(
        autocapitalizationType,
        setAutocapitalizationType,
        2_isize,
        0_isize
    );
    check!(spellCheckingType, setSpellCheckingType, 0_isize, 2_isize);
    check!(returnKeyType, setReturnKeyType, 0_isize, 9_isize);
    eprintln!("IOS_INPUT_TRAITS_LIVE_PROXY PASS (six checked setter/getter round-trips)");
}
