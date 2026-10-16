use super::ConnectivityStatus;
use objc2::{msg_send, runtime::AnyObject};
use std::ffi::c_void;

#[link(name = "Network", kind = "framework")]
extern "C" {
    fn nw_path_monitor_create() -> *mut AnyObject;
    fn nw_path_monitor_set_update_handler(
        monitor: *mut AnyObject,
        block: &block2::Block<dyn Fn(*mut c_void)>,
    );
    fn nw_path_monitor_set_queue(monitor: *mut AnyObject, queue: *mut c_void);
    fn nw_path_monitor_start(monitor: *mut AnyObject);
    fn nw_path_monitor_cancel(monitor: *mut AnyObject);
    fn nw_path_get_status(path: *mut c_void) -> i32;
    fn nw_path_uses_interface_type(path: *mut c_void, kind: i32) -> bool;
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
}

pub fn check_connectivity() -> ConnectivityStatus {
    unsafe {
        let monitor = nw_path_monitor_create();
        if monitor.is_null() {
            return ConnectivityStatus::None;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        let handler = block2::RcBlock::new(move |path: *mut c_void| {
            let status = super::classify_path(
                nw_path_get_status(path) == 1,
                nw_path_uses_interface_type(path, 1),
                nw_path_uses_interface_type(path, 2),
            );
            let _ = sender.send(status);
        });
        nw_path_monitor_set_update_handler(monitor, &handler);
        nw_path_monitor_set_queue(monitor, dispatch_get_global_queue(0, 0));
        nw_path_monitor_start(monitor);
        let status = receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap_or(ConnectivityStatus::None);
        nw_path_monitor_cancel(monitor);
        let _: () = msg_send![monitor, release];
        status
    }
}
