use super::CMTime;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

pub(super) unsafe fn require_playable(player: *mut AnyObject) -> Result<(), String> {
    let item: *mut AnyObject = msg_send![player, currentItem];
    if item.is_null() {
        return Err("No media source set".into());
    }
    for object in [player, item] {
        let status: isize = msg_send![object, status];
        if status == 2 {
            let error: *mut AnyObject = msg_send![object, error];
            if !error.is_null() {
                let description: *mut AnyObject = msg_send![error, localizedDescription];
                let text: *const std::ffi::c_char = msg_send![description, UTF8String];
                if !text.is_null() {
                    return Err(std::ffi::CStr::from_ptr(text)
                        .to_string_lossy()
                        .into_owned());
                }
            }
            return Err("Native media source failed".into());
        }
    }
    // Unknown status can be legitimate asynchronous buffering; only failure
    // and absence reject the existing play request.
    Ok(())
}

#[derive(Default)]
pub(super) struct Looping {
    looper: Option<std::ptr::NonNull<AnyObject>>,
}

impl Looping {
    pub unsafe fn clear(&mut self) {
        if let Some(looper) = self.looper.take() {
            let _: () = msg_send![looper.as_ptr(), disableLooping];
            let _: () = msg_send![looper.as_ptr(), release];
        }
    }

    // Caller serializes access and retains the queue player for this operation.
    pub unsafe fn configure(
        &mut self,
        player: *mut AnyObject,
        enabled: bool,
    ) -> Result<(), String> {
        let item: *mut AnyObject = msg_send![player, currentItem];
        if item.is_null() {
            return Err("No media source set".into());
        }
        let duration: CMTime = msg_send![item, duration];
        if enabled
            && duration.flags & 1 != 0
            && duration.flags & (4 | 8 | 16) == 0
            && duration.value == 0
        {
            return Err("Cannot loop an empty media item".into());
        }
        let time: CMTime = msg_send![player, currentTime];
        let rate: f32 = msg_send![player, rate];
        let _: *mut AnyObject = msg_send![item, retain];
        let _: () = msg_send![player, pause];
        self.clear();
        let _: () = msg_send![player, removeAllItems];
        if enabled {
            let looper: *mut AnyObject = msg_send![class!(AVPlayerLooper), alloc];
            let looper: *mut AnyObject =
                msg_send![looper, initWithPlayer: player, templateItem: item];
            self.looper = std::ptr::NonNull::new(looper);
        } else {
            let _: () =
                msg_send![player, insertItem: item, afterItem: std::ptr::null_mut::<AnyObject>()];
            let _: () = msg_send![player, setActionAtItemEnd: 1isize];
        }
        if enabled && self.looper.is_none() {
            let _: () =
                msg_send![player, insertItem: item, afterItem: std::ptr::null_mut::<AnyObject>()];
        }
        let _: () = msg_send![item, release];
        if time.flags & 1 != 0 && time.flags & (4 | 8 | 16) == 0 {
            let _: () = msg_send![player, seekToTime: time];
        }
        let _: () = msg_send![player, setRate: rate];
        if enabled && self.looper.is_none() {
            return Err("Failed to create AVPlayerLooper".into());
        }
        Ok(())
    }
}
