use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};

pub fn launch_url(url: &str) -> Result<bool, String> {
    if is_main() {
        return Err("Call launch_url from a worker thread".into());
    }
    futures::executor::block_on(launch_url_async(url))
}

pub async fn launch_url_async(url: &str) -> Result<bool, String> {
    let url = url.to_owned();
    let (sender, receiver) = futures::channel::oneshot::channel();
    on_main(move || unsafe {
        let nsurl = match nsurl_from_str(&url) {
            Ok(url) => url,
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        };
        let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        if app.is_null() {
            let _ = sender.send(Err("UIApplication unavailable".into()));
            return;
        }
        let sender = std::sync::Mutex::new(Some(sender));
        let completion = block2::RcBlock::new(move |success: Bool| {
            if let Some(sender) = sender.lock().unwrap().take() {
                let _ = sender.send(Ok(success.as_bool()));
            }
        });
        let options: *mut AnyObject = msg_send![class!(NSDictionary), dictionary];
        let _: () =
            msg_send![app, openURL: nsurl, options: options, completionHandler: &*completion];
    });
    receiver
        .await
        .map_err(|_| "URL completion disconnected".to_owned())?
}

pub fn can_launch_url(url: &str) -> Result<bool, String> {
    if is_main() {
        return unsafe { can_open(url) };
    }
    let url = url.to_owned();
    let (sender, receiver) = std::sync::mpsc::channel();
    on_main(move || {
        let _ = sender.send(unsafe { can_open(&url) });
    });
    receiver
        .recv()
        .map_err(|_| "URL query disconnected".to_owned())?
}

unsafe fn can_open(url: &str) -> Result<bool, String> {
    let nsurl = nsurl_from_str(url)?;
    let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
    if app.is_null() {
        return Err("UIApplication unavailable".into());
    }
    Ok(msg_send![app, canOpenURL: nsurl])
}

fn is_main() -> bool {
    unsafe { msg_send![class!(NSThread), isMainThread] }
}

fn on_main(f: impl FnOnce() + Send + 'static) {
    if is_main() {
        objc2::rc::autoreleasepool(|_| f());
        return;
    }
    extern "C" {
        fn dispatch_async_f(
            queue: *mut std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: unsafe extern "C" fn(*mut std::ffi::c_void),
        );
    }
    unsafe extern "C" fn run<F: FnOnce()>(context: *mut std::ffi::c_void) {
        let f = *Box::from_raw(context as *mut F);
        objc2::rc::autoreleasepool(|_| f());
    }
    unsafe fn enqueue<F: FnOnce() + Send + 'static>(f: F) {
        dispatch_async_f(
            crate::ios::dispatch_get_main_queue(),
            Box::into_raw(Box::new(f)).cast(),
            run::<F>,
        );
    }
    unsafe {
        enqueue(f);
    }
}

unsafe fn nsurl_from_str(url: &str) -> Result<*mut AnyObject, String> {
    let string = crate::ios::util::nsstring(url);
    let nsurl: *mut AnyObject = msg_send![class!(NSURL), URLWithString: string];
    if nsurl.is_null() {
        return Err(format!("Failed to parse URL: {url}"));
    }
    let scheme: *mut AnyObject = msg_send![nsurl, scheme];
    if scheme.is_null() {
        return Err("URL must include a scheme".into());
    }
    Ok(nsurl)
}
