use super::{
    CameraDescription, CameraHandle, CameraLensDirection, CapturedImage, ExposureMode, FlashMode,
    FocusMode, RecordedVideo, ResolutionPreset,
};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, Once};
use std::time::Duration;

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {}

// ── Session state ───────────────────────────────────────────────────────────

struct SessionState {
    operations: Arc<Mutex<()>>,
    session: *mut AnyObject,       // AVCaptureSession
    device: *mut AnyObject,        // AVCaptureDevice
    device_input: *mut AnyObject,  // AVCaptureDeviceInput
    photo_output: *mut AnyObject,  // AVCapturePhotoOutput
    video_output: *mut AnyObject,  // AVCaptureMovieFileOutput (may be null)
    _enable_audio: bool,
    _audio_input: *mut AnyObject, // AVCaptureDeviceInput for audio (may be null)
    flash_mode: FlashMode,
}

struct RetainedObject(*mut AnyObject);

impl RetainedObject {
    unsafe fn retain(object: *mut AnyObject) -> Self {
        Self(msg_send![object, retain])
    }

    fn as_ptr(&self) -> *mut AnyObject {
        self.0
    }
}

impl Drop for RetainedObject {
    fn drop(&mut self) {
        unsafe { release(self.0) }
    }
}

// SAFETY: RetainedObject owns one ObjC retain and only releases it on drop.
unsafe impl Send for RetainedObject {}

// SAFETY: all pointers are ObjC objects accessed on the main thread or under lock
unsafe impl Send for SessionState {}

impl Drop for SessionState {
    fn drop(&mut self) {
        unsafe {
            release(self.video_output);
            release(self.photo_output);
            release(self.device_input);
            release(self._audio_input);
            release(self.device);
            release(self.session);
        }
    }
}

static SESSIONS: Mutex<Option<HashMap<usize, SessionState>>> = Mutex::new(None);
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

fn sessions() -> std::sync::MutexGuard<'static, Option<HashMap<usize, SessionState>>> {
    let mut guard = SESSIONS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

fn camera_operations(id: usize) -> Result<Arc<Mutex<()>>, String> {
    sessions().as_ref().unwrap().get(&id)
        .map(|state| Arc::clone(&state.operations))
        .ok_or_else(|| "Invalid camera handle".into())
}

fn next_id() -> usize {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

unsafe fn release(object: *mut AnyObject) {
    if !object.is_null() {
        let _: () = msg_send![object, release];
    }
}

/// Retain the AVCaptureSession for a session ID.
///
/// The caller owns the returned reference and must release it.
pub fn get_session_ptr(id: usize) -> Option<*mut AnyObject> {
    let guard = sessions();
    guard
        .as_ref()
        .and_then(|map| {
            map.get(&id)
                .map(|state| unsafe { msg_send![state.session, retain] })
        })
}

/// Retain the AVCaptureSession while the registry lock is held. The caller owns
/// the returned reference and must release it after using the session.
pub fn retain_session_ptr(id: usize) -> Option<*mut AnyObject> {
    get_session_ptr(id)
}

// ── Photo capture delegate ──────────────────────────────────────────────────

struct PhotoRequest {
    sender: Option<mpsc::Sender<Result<CapturedImage, String>>>,
    delegate: usize,
    _session: RetainedObject,
    _output: RetainedObject,
}

static PHOTO_REQUESTS: Mutex<Option<HashMap<usize, PhotoRequest>>> = Mutex::new(None);
const PHOTO_CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

fn photo_requests() -> std::sync::MutexGuard<'static, Option<HashMap<usize, PhotoRequest>>> {
    let mut guard = PHOTO_REQUESTS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

/// Wait for one native request. A timeout does not cancel the request; a late
/// callback still finds its registered sender and releases its delegate.
fn recv_bounded<T>(
    receiver: mpsc::Receiver<Result<T, String>>,
    timeout: Duration,
    operation: &str,
) -> Result<T, String> {
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!("{operation} timed out")),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(format!("{operation} completion channel closed"))
        }
    }
}

fn after_finalization<T: Send + 'static>(
    receiver: mpsc::Receiver<T>,
    cleanup: impl FnOnce() + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _ = receiver.recv();
        cleanup();
    })
}

static REGISTER_PHOTO_DELEGATE: Once = Once::new();
static mut PHOTO_DELEGATE_CLASS: *const AnyClass = std::ptr::null();

fn photo_delegate_class() -> &'static AnyClass {
    REGISTER_PHOTO_DELEGATE.call_once(|| {
        let superclass = class!(NSObject);
        let mut decl = ClassBuilder::new(c"GpuiPhotoCaptureDelegate", superclass).unwrap();

        unsafe {
            // captureOutput:didFinishProcessingPhoto:error:
            decl.add_method(
                sel!(captureOutput:didFinishProcessingPhoto:error:),
                photo_did_finish
                    as extern "C" fn(
                        *mut AnyObject,
                        Sel,
                        *mut AnyObject,
                        *mut AnyObject,
                        *mut AnyObject,
                    ),
            );
            // This callback is guaranteed to arrive last for every request.
            decl.add_method(
                sel!(captureOutput:didFinishCaptureForResolvedSettings:error:),
                photo_did_finish_capture
                    as extern "C" fn(
                        *mut AnyObject,
                        Sel,
                        *mut AnyObject,
                        *mut AnyObject,
                        *mut AnyObject,
                    ),
            );
        }

        unsafe { PHOTO_DELEGATE_CLASS = decl.register() as *const AnyClass };
    });
    unsafe { &*PHOTO_DELEGATE_CLASS }
}

extern "C" fn photo_did_finish(
    _this: *mut AnyObject,
    _sel: Sel,
    _output: *mut AnyObject,
    photo: *mut AnyObject,
    error: *mut AnyObject,
) {
    let result = unsafe {
        if !error.is_null() {
            let desc: *mut AnyObject = msg_send![error, localizedDescription];
            let cstr: *const std::ffi::c_char = msg_send![desc, UTF8String];
            let msg = if !cstr.is_null() {
                std::ffi::CStr::from_ptr(cstr)
                    .to_string_lossy()
                    .into_owned()
            } else {
                "Unknown capture error".to_string()
            };
            Err(msg)
        } else {
            // Get JPEG data
            let jpeg_data: *mut AnyObject = msg_send![photo, fileDataRepresentation];
            if jpeg_data.is_null() {
                Err("Failed to get photo data".to_string())
            } else {
                // Save to temp file
                let uuid_str = uuid::Uuid::new_v4().to_string();
                let file_name = format!("camera_{}.jpg", uuid_str);
                let file_path = std::env::temp_dir().join(&file_name);
                let ns_path = nsstring(&file_path.to_string_lossy());
                let wrote: Bool = msg_send![jpeg_data, writeToFile: ns_path, atomically: true];

                if wrote.as_bool() {
                    // Get dimensions from CGImage
                    let cg_image: *mut AnyObject = msg_send![photo, CGImageRepresentation];
                    let (width, height) = if !cg_image.is_null() {
                        let w: usize = CGImageGetWidth(cg_image);
                        let h: usize = CGImageGetHeight(cg_image);
                        (w as u32, h as u32)
                    } else {
                        (0, 0)
                    };

                    Ok(CapturedImage {
                        path: file_path.to_string_lossy().into_owned(),
                        width,
                        height,
                    })
                } else {
                    Err("Failed to write photo to file".to_string())
                }
            }
        }
    };

    let sender = photo_requests()
        .as_mut()
        .and_then(|requests| requests.get_mut(&(_this as usize)))
        .and_then(|request| request.sender.take());
    if let Some(sender) = sender {
        let _ = sender.send(result);
    }
}

extern "C" fn photo_did_finish_capture(
    _this: *mut AnyObject,
    _sel: Sel,
    _output: *mut AnyObject,
    _resolved_settings: *mut AnyObject,
    _error: *mut AnyObject,
) {
    // AVFoundation guarantees this callback is last. Releasing the delegate
    // here is safe; no later callback can use it.
    let request = photo_requests()
        .as_mut()
        .and_then(|requests| requests.remove(&(_this as usize)));
    if let Some(request) = request {
        if let Some(sender) = request.sender {
            let _ = sender.send(Err("Photo capture finished without an image".into()));
        }
        unsafe { release(request.delegate as *mut AnyObject) };
    }
}

extern "C" {
    fn CGImageGetWidth(image: *mut AnyObject) -> usize;
    fn CGImageGetHeight(image: *mut AnyObject) -> usize;
}

// ── Video recording delegate ────────────────────────────────────────────────

struct VideoRequest {
    sender: mpsc::Sender<Result<RecordedVideo, String>>,
    delegate: usize,
}

static VIDEO_REQUESTS: Mutex<Option<HashMap<usize, VideoRequest>>> = Mutex::new(None);
static VIDEO_RECEIVERS: Mutex<
    Option<HashMap<usize, mpsc::Receiver<Result<RecordedVideo, String>>>>,
> = Mutex::new(None);
const VIDEO_CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);

fn video_requests() -> std::sync::MutexGuard<'static, Option<HashMap<usize, VideoRequest>>> {
    let mut guard = VIDEO_REQUESTS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

fn video_receivers() -> std::sync::MutexGuard<
    'static,
    Option<HashMap<usize, mpsc::Receiver<Result<RecordedVideo, String>>>>,
> {
    let mut guard = VIDEO_RECEIVERS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

static REGISTER_VIDEO_DELEGATE: Once = Once::new();
static mut VIDEO_DELEGATE_CLASS: *const AnyClass = std::ptr::null();

fn video_delegate_class() -> &'static AnyClass {
    REGISTER_VIDEO_DELEGATE.call_once(|| {
        let superclass = class!(NSObject);
        let mut decl = ClassBuilder::new(c"GpuiVideoRecordingDelegate", superclass).unwrap();

        unsafe {
            // captureOutput:didFinishRecordingToOutputFileAtURL:fromConnections:error:
            decl.add_method(
                sel!(captureOutput:didFinishRecordingToOutputFileAtURL:fromConnections:error:),
                video_did_finish
                    as extern "C" fn(
                        *mut AnyObject,
                        Sel,
                        *mut AnyObject,
                        *mut AnyObject,
                        *mut AnyObject,
                        *mut AnyObject,
                    ),
            );
        }

        unsafe { VIDEO_DELEGATE_CLASS = decl.register() as *const AnyClass };
    });
    unsafe { &*VIDEO_DELEGATE_CLASS }
}

extern "C" fn video_did_finish(
    _this: *mut AnyObject,
    _sel: Sel,
    _output: *mut AnyObject,
    url: *mut AnyObject,
    _connections: *mut AnyObject,
    error: *mut AnyObject,
) {
    let result = unsafe {
        if !error.is_null() {
            let desc: *mut AnyObject = msg_send![error, localizedDescription];
            let cstr: *const std::ffi::c_char = msg_send![desc, UTF8String];
            let msg = if !cstr.is_null() {
                std::ffi::CStr::from_ptr(cstr)
                    .to_string_lossy()
                    .into_owned()
            } else {
                "Unknown recording error".to_string()
            };
            Err(msg)
        } else if url.is_null() {
            Err("No output URL".to_string())
        } else {
            let path_obj: *mut AnyObject = msg_send![url, path];
            let cstr: *const std::ffi::c_char = msg_send![path_obj, UTF8String];
            if !cstr.is_null() {
                let path = std::ffi::CStr::from_ptr(cstr)
                    .to_string_lossy()
                    .into_owned();
                Ok(RecordedVideo { path })
            } else {
                Err("Failed to get video path".to_string())
            }
        }
    };

    let request = video_requests()
        .as_mut()
        .and_then(|requests| requests.remove(&(_this as usize)));
    if let Some(request) = request {
        let _ = request.sender.send(result);
        unsafe { release(request.delegate as *mut AnyObject) };
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

use crate::ios::util::nsstring;

fn av_flash_mode(mode: FlashMode) -> i64 {
    match mode {
        FlashMode::Always => 1, // AVCaptureFlashModeOn
        FlashMode::Auto => 2,   // AVCaptureFlashModeAuto
        FlashMode::Off | FlashMode::Torch => 0, // AVCaptureFlashModeOff
    }
}

fn resolution_to_preset(resolution: ResolutionPreset) -> &'static str {
    match resolution {
        ResolutionPreset::Low => "AVCaptureSessionPresetLow",
        ResolutionPreset::Medium => "AVCaptureSessionPresetMedium",
        ResolutionPreset::High => "AVCaptureSessionPreset1280x720",
        ResolutionPreset::VeryHigh => "AVCaptureSessionPreset1920x1080",
        ResolutionPreset::UltraHigh => "AVCaptureSessionPreset3840x2160",
        ResolutionPreset::Max => "AVCaptureSessionPresetPhoto",
    }
}

unsafe fn find_device_by_name(name: &str) -> *mut AnyObject {
    // Use AVCaptureDeviceDiscoverySession to find all cameras
    let device_types: *mut AnyObject = {
        let wide = nsstring("AVCaptureDeviceTypeBuiltInWideAngleCamera");
        let ultra = nsstring("AVCaptureDeviceTypeBuiltInUltraWideCamera");
        let tele = nsstring("AVCaptureDeviceTypeBuiltInTelephotoCamera");
        msg_send![class!(NSArray),
            arrayWithObjects: [wide, ultra, tele].as_ptr(),
            count: 3usize
        ]
    };

    let media_type = nsstring("vide"); // AVMediaTypeVideo
    let position: i64 = 0; // AVCaptureDevicePositionUnspecified

    let discovery: *mut AnyObject = msg_send![class!(AVCaptureDeviceDiscoverySession),
        discoverySessionWithDeviceTypes: device_types,
        mediaType: media_type,
        position: position
    ];

    if discovery.is_null() {
        return std::ptr::null_mut();
    }

    let devices: *mut AnyObject = msg_send![discovery, devices];
    let count: usize = msg_send![devices, count];

    for i in 0..count {
        let device: *mut AnyObject = msg_send![devices, objectAtIndex: i];
        let unique_id: *mut AnyObject = msg_send![device, uniqueID];
        let cstr: *const std::ffi::c_char = msg_send![unique_id, UTF8String];
        if !cstr.is_null() {
            let id = std::ffi::CStr::from_ptr(cstr).to_string_lossy();
            if id == name {
                return device;
            }
        }
    }

    std::ptr::null_mut()
}

// ── Public API ──────────────────────────────────────────────────────────────

pub fn available_cameras() -> Result<Vec<CameraDescription>, String> {
    unsafe {
        // AVCaptureDeviceType is an NSString typedef, not an ObjC class.
        // Use the string constants directly.
        let device_types: *mut AnyObject = {
            let wide = nsstring("AVCaptureDeviceTypeBuiltInWideAngleCamera");
            let ultra = nsstring("AVCaptureDeviceTypeBuiltInUltraWideCamera");
            let tele = nsstring("AVCaptureDeviceTypeBuiltInTelephotoCamera");
            msg_send![class!(NSArray),
                arrayWithObjects: [wide, ultra, tele].as_ptr(),
                count: 3usize
            ]
        };

        let media_type = nsstring("vide"); // AVMediaTypeVideo
        let position: i64 = 0; // AVCaptureDevicePositionUnspecified

        let discovery: *mut AnyObject = msg_send![class!(AVCaptureDeviceDiscoverySession),
            discoverySessionWithDeviceTypes: device_types,
            mediaType: media_type,
            position: position
        ];

        if discovery.is_null() {
            return Ok(vec![]);
        }

        let devices: *mut AnyObject = msg_send![discovery, devices];
        let count: usize = msg_send![devices, count];
        let mut cameras = Vec::with_capacity(count);

        for i in 0..count {
            let device: *mut AnyObject = msg_send![devices, objectAtIndex: i];

            // Get unique ID
            let unique_id: *mut AnyObject = msg_send![device, uniqueID];
            let cstr: *const std::ffi::c_char = msg_send![unique_id, UTF8String];
            let name = if !cstr.is_null() {
                std::ffi::CStr::from_ptr(cstr)
                    .to_string_lossy()
                    .into_owned()
            } else {
                format!("camera_{i}")
            };

            // Get position -> lens direction
            let position: i64 = msg_send![device, position];
            let lens_direction = match position {
                1 => CameraLensDirection::Back,
                2 => CameraLensDirection::Front,
                _ => CameraLensDirection::External,
            };

            cameras.push(CameraDescription {
                name,
                lens_direction,
                sensor_orientation: 0, // iOS handles rotation automatically
            });
        }

        Ok(cameras)
    }
}

pub fn create_camera(
    camera: &CameraDescription,
    resolution: ResolutionPreset,
    enable_audio: bool,
) -> Result<usize, String> {
    unsafe {
        // Find the device
        let device = find_device_by_name(&camera.name);
        if device.is_null() {
            return Err(format!("Camera '{}' not found", camera.name));
        }
        let device: *mut AnyObject = msg_send![device, retain];

        // Create session
        let session: *mut AnyObject = msg_send![class!(AVCaptureSession), alloc];
        let session: *mut AnyObject = msg_send![session, init];
        if session.is_null() {
            release(device);
            return Err("Failed to create AVCaptureSession".into());
        }

        // Begin configuration
        let _: () = msg_send![session, beginConfiguration];

        // Set resolution preset
        let preset = nsstring(resolution_to_preset(resolution));
        let can_set: Bool = msg_send![session, canSetSessionPreset: preset];
        if can_set.as_bool() {
            let _: () = msg_send![session, setSessionPreset: preset];
        }

        // Add video input
        let mut error_ptr: *mut AnyObject = std::ptr::null_mut();
        let input: *mut AnyObject = msg_send![class!(AVCaptureDeviceInput),
            deviceInputWithDevice: device,
            error: &mut error_ptr
        ];
        if input.is_null() || !error_ptr.is_null() {
            let _: () = msg_send![session, commitConfiguration];
            release(device);
            release(session);
            return Err("Failed to create device input".into());
        }
        let input: *mut AnyObject = msg_send![input, retain];

        let can_add: Bool = msg_send![session, canAddInput: input];
        if !can_add.as_bool() {
            let _: () = msg_send![session, commitConfiguration];
            release(input);
            release(device);
            release(session);
            return Err("Cannot add camera input to session".into());
        }
        let _: () = msg_send![session, addInput: input];

        // Add audio input if requested
        let mut audio_input: *mut AnyObject = std::ptr::null_mut();
        if enable_audio {
            let audio_type = nsstring("soun"); // AVMediaTypeAudio
            let audio_device: *mut AnyObject = msg_send![class!(AVCaptureDevice),
                defaultDeviceWithMediaType: audio_type
            ];
            if !audio_device.is_null() {
                let mut audio_err: *mut AnyObject = std::ptr::null_mut();
                let a_input: *mut AnyObject = msg_send![class!(AVCaptureDeviceInput),
                    deviceInputWithDevice: audio_device,
                    error: &mut audio_err
                ];
                if !a_input.is_null() && audio_err.is_null() {
                    let a_input: *mut AnyObject = msg_send![a_input, retain];
                    let can_add_audio: Bool = msg_send![session, canAddInput: a_input];
                    if can_add_audio.as_bool() {
                        let _: () = msg_send![session, addInput: a_input];
                        audio_input = a_input;
                    } else {
                        release(a_input);
                    }
                }
            }
        }

        // Add photo output
        let photo_output: *mut AnyObject = msg_send![class!(AVCapturePhotoOutput), alloc];
        let photo_output: *mut AnyObject = msg_send![photo_output, init];
        if photo_output.is_null() {
            let _: () = msg_send![session, commitConfiguration];
            release(audio_input);
            release(input);
            release(device);
            release(session);
            return Err("Failed to create photo output".into());
        }
        let can_add_photo: Bool = msg_send![session, canAddOutput: photo_output];
        if !can_add_photo.as_bool() {
            let _: () = msg_send![session, commitConfiguration];
            release(photo_output);
            release(audio_input);
            release(input);
            release(device);
            release(session);
            return Err("Cannot add photo output to session".into());
        }
        let _: () = msg_send![session, addOutput: photo_output];

        // Add video file output
        let mut video_output: *mut AnyObject = msg_send![class!(AVCaptureMovieFileOutput), alloc];
        video_output = msg_send![video_output, init];
        if !video_output.is_null() {
            let can_add_video: Bool = msg_send![session, canAddOutput: video_output];
            if can_add_video.as_bool() {
                let _: () = msg_send![session, addOutput: video_output];
            } else {
                // Video recording won't be available, but that's okay.
                log::warn!("Cannot add video output to session");
                release(video_output);
                video_output = std::ptr::null_mut();
            }
        }

        // Commit configuration
        let _: () = msg_send![session, commitConfiguration];

        let id = next_id();
        sessions().as_mut().unwrap().insert(
            id,
            SessionState {
                operations: Arc::new(Mutex::new(())),
                session,
                device,
                device_input: input,
                photo_output,
                video_output,
                _enable_audio: enable_audio,
                _audio_input: audio_input,
                flash_mode: FlashMode::Off,
            },
        );

        Ok(id)
    }
}

pub fn start_preview_session(handle: &CameraHandle) -> Result<(), String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    let session = {
        let guard = sessions();
        let state = guard
            .as_ref()
            .unwrap()
            .get(&handle.id)
            .ok_or("Invalid camera handle")?;
        unsafe { RetainedObject::retain(state.session) }
    };
    unsafe {
        let running: Bool = msg_send![session.as_ptr(), isRunning];
        if !running.as_bool() {
            // startRunning is synchronous; callers must start preview off the
            // iOS main thread. The registry lock is not held here.
            let _: () = msg_send![session.as_ptr(), startRunning];
        }
        let running: Bool = msg_send![session.as_ptr(), isRunning];
        if !running.as_bool() {
            return Err("Camera session failed to start".into());
        }
    }
    Ok(())
}

pub fn stop_preview_session(handle: &CameraHandle) -> Result<(), String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    if video_receivers().as_ref().unwrap().contains_key(&handle.id) {
        return Err("Finish video recording before stopping the preview".into());
    }
    let session = {
        let guard = sessions();
        let state = guard
            .as_ref()
            .unwrap()
            .get(&handle.id)
            .ok_or("Invalid camera handle")?;
        unsafe { RetainedObject::retain(state.session) }
    };
    unsafe {
        let running: Bool = msg_send![session.as_ptr(), isRunning];
        if running.as_bool() {
            let _: () = msg_send![session.as_ptr(), stopRunning];
        }
    }
    Ok(())
}

pub fn take_picture(handle: &CameraHandle) -> Result<CapturedImage, String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    let (tx, rx) = mpsc::channel();

    let (session, output, flash_mode) = {
        let guard = sessions();
        let state = guard
            .as_ref()
            .unwrap()
            .get(&handle.id)
            .ok_or("Invalid camera handle")?;

        let running: Bool = unsafe { msg_send![state.session, isRunning] };
        if !running.as_bool() {
            return Err("Camera session is not running".into());
        }
        if state.photo_output.is_null() {
            return Err("Photo output not available".into());
        }
        (
            unsafe { RetainedObject::retain(state.session) },
            unsafe { RetainedObject::retain(state.photo_output) },
            state.flash_mode,
        )
    };

    unsafe {
        // Create photo settings
        let settings: *mut AnyObject = msg_send![class!(AVCapturePhotoSettings), photoSettings];
        if settings.is_null() {
            return Err("Failed to create photo settings".into());
        }
        let flash_mode = av_flash_mode(flash_mode);
        let _: () = msg_send![settings, setFlashMode: flash_mode];

        // Create delegate
        let delegate_cls = photo_delegate_class();
        let delegate: *mut AnyObject = msg_send![delegate_cls, alloc];
        let delegate: *mut AnyObject = msg_send![delegate, init];
        if delegate.is_null() {
            return Err("Failed to create photo capture delegate".into());
        }
        let key = delegate as usize;
        let output_ptr = output.as_ptr();
        photo_requests().as_mut().unwrap().insert(
            key,
            PhotoRequest {
                sender: Some(tx),
                delegate: key,
                _session: session,
                _output: output,
            },
        );

        // Capture
        let _: () = msg_send![output_ptr,
            capturePhotoWithSettings: settings,
            delegate: delegate
        ];
    }

    recv_bounded(rx, PHOTO_CAPTURE_TIMEOUT, "Photo capture")
}

pub fn start_video_recording(handle: &CameraHandle) -> Result<(), String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    unsafe {
        let guard = sessions();
        let state = guard
            .as_ref()
            .unwrap()
            .get(&handle.id)
            .ok_or("Invalid camera handle")?;

        let running: Bool = msg_send![state.session, isRunning];
        if !running.as_bool() {
            return Err("Camera session is not running".into());
        }
        if state.video_output.is_null() {
            return Err("Video output not available".into());
        }

        let is_recording: Bool = msg_send![state.video_output, isRecording];
        if is_recording.as_bool() {
            return Err("Already recording".into());
        }

        // Create output file URL
        let uuid_str = uuid::Uuid::new_v4().to_string();
        let file_name = format!("video_{}.mov", uuid_str);
        let file_path = std::env::temp_dir().join(&file_name);
        let ns_path = nsstring(&file_path.to_string_lossy());
        let file_url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: ns_path];

        // Create delegate
        let delegate_cls = video_delegate_class();
        let delegate: *mut AnyObject = msg_send![delegate_cls, alloc];
        let delegate: *mut AnyObject = msg_send![delegate, init];
        if delegate.is_null() {
            return Err("Failed to create video recording delegate".into());
        }
        let key = delegate as usize;
        let (tx, rx) = mpsc::channel();
        {
            let mut receivers = video_receivers();
            let receivers = receivers.as_mut().unwrap();
            if receivers.contains_key(&handle.id) {
                release(delegate);
                return Err("Video recording is already pending".into());
            }
            receivers.insert(handle.id, rx);
        }
        video_requests().as_mut().unwrap().insert(
            key,
            VideoRequest {
                sender: tx,
                delegate: key,
            },
        );

        let _: () = msg_send![state.video_output,
            startRecordingToOutputFileURL: file_url,
            recordingDelegate: delegate
        ];

        Ok(())
    }
}

pub fn stop_video_recording(handle: &CameraHandle) -> Result<RecordedVideo, String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    let receiver = unsafe {
        let guard = sessions();
        let state = guard
            .as_ref()
            .unwrap()
            .get(&handle.id)
            .ok_or("Invalid camera handle")?;

        if state.video_output.is_null() {
            return Err("Video output not available".into());
        }

        let receiver = video_receivers()
            .as_mut()
            .unwrap()
            .remove(&handle.id)
            .ok_or("No video recording in progress")?;
        let is_recording: Bool = msg_send![state.video_output, isRecording];
        if is_recording.as_bool() {
            let _: () = msg_send![state.video_output, stopRecording];
        }
        receiver
    };

    match receiver.recv_timeout(VIDEO_CAPTURE_TIMEOUT) {
        Ok(result) => result,
        Err(error) => {
            // Keep the receiver available for dispose to wait for finalization.
            let guard = sessions();
            if guard
                .as_ref()
                .is_some_and(|sessions| sessions.contains_key(&handle.id))
            {
                video_receivers()
                    .as_mut()
                    .unwrap()
                    .insert(handle.id, receiver);
            }
            drop(guard);
            Err(format!("Video recording did not complete: {error}"))
        }
    }
}

pub fn set_flash_mode(handle: &CameraHandle, mode: FlashMode) -> Result<(), String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    unsafe {
        let mut guard = sessions();
        let state = guard
            .as_mut()
            .unwrap()
            .get_mut(&handle.id)
            .ok_or("Invalid camera handle")?;

        let device = state.device;

        // For torch mode, use torchMode; for flash, it's set on capture settings
        if mode == FlashMode::Torch {
            let has_torch: Bool = msg_send![device, hasTorch];
            if !has_torch.as_bool() {
                return Err("Device does not have a torch".into());
            }
            let mut err: *mut AnyObject = std::ptr::null_mut();
            let locked: Bool =
                msg_send![device, lockForConfiguration: &mut err as *mut *mut AnyObject];
            if !locked.as_bool() {
                return Err("Failed to lock device for configuration".into());
            }
            let _: () = msg_send![device, setTorchMode: 1i64]; // AVCaptureTorchModeOn
            let _: () = msg_send![device, unlockForConfiguration];
            state.flash_mode = FlashMode::Off;
            return Ok(());
        } else {
            if mode != FlashMode::Off {
                let has_flash: Bool = msg_send![device, hasFlash];
                if !has_flash.as_bool() {
                    return Err("Device does not have a flash".into());
                }
                let avf_mode = av_flash_mode(mode);
                let supported: Bool = msg_send![device, isFlashModeSupported: avf_mode];
                if !supported.as_bool() {
                    return Err("Flash mode not supported".into());
                }
            }
            // Turn off torch if it was on
            let has_torch: Bool = msg_send![device, hasTorch];
            if has_torch.as_bool() {
                let torch_mode: i64 = msg_send![device, torchMode];
                if torch_mode != 0 {
                    let mut err: *mut AnyObject = std::ptr::null_mut();
                    let locked: Bool =
                        msg_send![device, lockForConfiguration: &mut err as *mut *mut AnyObject];
                    if locked.as_bool() {
                        let _: () = msg_send![device, setTorchMode: 0i64]; // AVCaptureTorchModeOff
                        let _: () = msg_send![device, unlockForConfiguration];
                    }
                }
            }
            state.flash_mode = mode;
        }

        Ok(())
    }
}

pub fn set_focus_mode(handle: &CameraHandle, mode: FocusMode) -> Result<(), String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    unsafe {
        let guard = sessions();
        let state = guard
            .as_ref()
            .unwrap()
            .get(&handle.id)
            .ok_or("Invalid camera handle")?;

        let device = state.device;
        let avf_mode: i64 = match mode {
            FocusMode::Auto => 2,   // AVCaptureFocusModeContinuousAutoFocus
            FocusMode::Locked => 0, // AVCaptureFocusModeLocked
        };

        let is_supported: Bool = msg_send![device, isFocusModeSupported: avf_mode];
        if !is_supported.as_bool() {
            return Err("Focus mode not supported".into());
        }

        let mut err: *mut AnyObject = std::ptr::null_mut();
        let locked: Bool = msg_send![device, lockForConfiguration: &mut err as *mut *mut AnyObject];
        if !locked.as_bool() {
            return Err("Failed to lock device for configuration".into());
        }

        let _: () = msg_send![device, setFocusMode: avf_mode];
        let _: () = msg_send![device, unlockForConfiguration];

        Ok(())
    }
}

pub fn set_exposure_mode(handle: &CameraHandle, mode: ExposureMode) -> Result<(), String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    unsafe {
        let guard = sessions();
        let state = guard
            .as_ref()
            .unwrap()
            .get(&handle.id)
            .ok_or("Invalid camera handle")?;

        let device = state.device;
        let avf_mode: i64 = match mode {
            ExposureMode::Auto => 2,   // AVCaptureExposureModeContinuousAutoExposure
            ExposureMode::Locked => 0, // AVCaptureExposureModeLocked
        };

        let is_supported: Bool = msg_send![device, isExposureModeSupported: avf_mode];
        if !is_supported.as_bool() {
            return Err("Exposure mode not supported".into());
        }

        let mut err: *mut AnyObject = std::ptr::null_mut();
        let locked: Bool = msg_send![device, lockForConfiguration: &mut err as *mut *mut AnyObject];
        if !locked.as_bool() {
            return Err("Failed to lock device for configuration".into());
        }

        let _: () = msg_send![device, setExposureMode: avf_mode];
        let _: () = msg_send![device, unlockForConfiguration];

        Ok(())
    }
}

pub fn get_min_zoom(_handle: &CameraHandle) -> Result<f64, String> {
    // AVCaptureDevice minimum zoom is always 1.0
    Ok(1.0)
}

pub fn get_max_zoom(handle: &CameraHandle) -> Result<f64, String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    unsafe {
        let guard = sessions();
        let state = guard
            .as_ref()
            .unwrap()
            .get(&handle.id)
            .ok_or("Invalid camera handle")?;

        let max_zoom: f64 = msg_send![state.device, maxAvailableVideoZoomFactor];
        Ok(max_zoom)
    }
}

pub fn set_zoom(handle: &CameraHandle, zoom: f64) -> Result<(), String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    unsafe {
        let guard = sessions();
        let state = guard
            .as_ref()
            .unwrap()
            .get(&handle.id)
            .ok_or("Invalid camera handle")?;

        let device = state.device;
        let max_zoom: f64 = msg_send![device, maxAvailableVideoZoomFactor];
        let clamped = zoom.max(1.0).min(max_zoom);

        let mut err: *mut AnyObject = std::ptr::null_mut();
        let locked: Bool = msg_send![device, lockForConfiguration: &mut err as *mut *mut AnyObject];
        if !locked.as_bool() {
            return Err("Failed to lock device for configuration".into());
        }

        let _: () = msg_send![device, setVideoZoomFactor: clamped];
        let _: () = msg_send![device, unlockForConfiguration];

        Ok(())
    }
}

pub fn set_camera(handle: &CameraHandle, camera: &CameraDescription) -> Result<(), String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    if video_receivers().as_ref().unwrap().contains_key(&handle.id) {
        return Err("Finish video recording before switching cameras".into());
    }
    unsafe {
        let new_device = find_device_by_name(&camera.name);
        if new_device.is_null() {
            return Err(format!("Camera '{}' not found", camera.name));
        }
        let new_device: *mut AnyObject = msg_send![new_device, retain];

        let mut guard = sessions();
        let state = match guard.as_mut().unwrap().get_mut(&handle.id) {
            Some(state) => state,
            None => {
                release(new_device);
                return Err("Invalid camera handle".into());
            }
        };

        let session = state.session;
        let old_device = state.device;
        let old_input = state.device_input;

        let _: () = msg_send![session, beginConfiguration];

        // Remove old input
        let _: () = msg_send![session, removeInput: old_input];

        // Create new input
        let mut error_ptr: *mut AnyObject = std::ptr::null_mut();
        let new_input: *mut AnyObject = msg_send![class!(AVCaptureDeviceInput),
            deviceInputWithDevice: new_device,
            error: &mut error_ptr as *mut *mut AnyObject
        ];

        if new_input.is_null() || !error_ptr.is_null() {
            // Rollback: re-add old input
            let _: () = msg_send![session, addInput: old_input];
            let _: () = msg_send![session, commitConfiguration];
            release(new_device);
            return Err("Failed to create input for new camera".into());
        }
        let new_input: *mut AnyObject = msg_send![new_input, retain];

        let can_add: Bool = msg_send![session, canAddInput: new_input];
        if !can_add.as_bool() {
            let _: () = msg_send![session, addInput: old_input];
            let _: () = msg_send![session, commitConfiguration];
            release(new_input);
            release(new_device);
            return Err("Cannot add new camera input".into());
        }

        let _: () = msg_send![session, addInput: new_input];

        state.device = new_device;
        state.device_input = new_input;
        state.flash_mode = FlashMode::Off;

        let _: () = msg_send![session, commitConfiguration];
        release(old_input);
        release(old_device);

        Ok(())
    }
}

pub fn dispose(handle: CameraHandle) -> Result<(), String> {
    let operations = camera_operations(handle.id)?;
    let _operation = operations.lock().unwrap();
    let (video_output, recording, video_result) = {
        let guard = sessions();
        let state = guard
            .as_ref()
            .unwrap()
            .get(&handle.id)
            .ok_or("Invalid camera handle")?;
        let recording = if state.video_output.is_null() {
            false
        } else {
            let recording: Bool = unsafe { msg_send![state.video_output, isRecording] };
            recording.as_bool()
        };
        let video_result = video_receivers()
            .as_mut()
            .unwrap()
            .remove(&handle.id);
        (state.video_output, recording, video_result)
    };

    if recording && video_result.is_none() {
        return Err("Cannot finalize recording without a completion channel".into());
    }

    if recording {
        unsafe {
            let _: () = msg_send![video_output, stopRecording];
        }
    }

    if let Some(video_result) = video_result {
        match video_result.recv_timeout(VIDEO_CAPTURE_TIMEOUT) {
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let state = sessions().as_mut().unwrap().remove(&handle.id);
                if let Some(state) = state {
                    let operations = Arc::clone(&state.operations);
                    after_finalization(video_result, move || {
                        // shortcut: retain native resources until AVFoundation's final callback;
                        // add cancellation if the platform exposes a safe cancellation contract.
                        let _operation = operations.lock().unwrap();
                        unsafe {
                            let running: Bool = msg_send![state.session, isRunning];
                            if running.as_bool() {
                                let _: () = msg_send![state.session, stopRunning];
                            }
                        }
                        drop(state);
                    });
                }
                return Err("Video finalization timed out; cleanup continues in background".into());
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
    }

    let state = {
        let mut guard = sessions();
        guard.as_mut().unwrap().remove(&handle.id)
    };

    if let Some(state) = state {
        unsafe {
            let running: Bool = msg_send![state.session, isRunning];
            if running.as_bool() {
                let _: () = msg_send![state.session, stopRunning];
            }
        }
        // SessionState::drop releases the retained AVFoundation objects.
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_operation_locks_are_independent() {
        let make_state = || SessionState {
            operations: Arc::new(Mutex::new(())),
            session: std::ptr::null_mut(),
            device: std::ptr::null_mut(),
            device_input: std::ptr::null_mut(),
            photo_output: std::ptr::null_mut(),
            video_output: std::ptr::null_mut(),
            _enable_audio: false,
            _audio_input: std::ptr::null_mut(),
            flash_mode: FlashMode::Off,
        };
        let (first, second) = (next_id(), next_id());
        {
            let mut guard = sessions();
            let map = guard.as_mut().unwrap();
            map.insert(first, make_state());
            map.insert(second, make_state());
        }
        let a = camera_operations(first).unwrap();
        let b = camera_operations(second).unwrap();
        let _held = a.lock().unwrap();
        assert!(a.try_lock().is_err());
        assert!(b.try_lock().is_ok());
        let mut guard = sessions();
        guard.as_mut().unwrap().remove(&first);
        guard.as_mut().unwrap().remove(&second);
    }

    #[test]
    fn deferred_cleanup_waits_for_recording_callback() {
        let (finished_tx, finished_rx) = mpsc::channel();
        let (cleaned_tx, cleaned_rx) = mpsc::channel();
        let worker = after_finalization(finished_rx, move || cleaned_tx.send(()).unwrap());
        assert!(matches!(cleaned_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        finished_tx.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(cleaned_rx.try_recv(), Ok(()));
    }

    #[test]
    fn photo_delegate_registers_final_callback() {
        assert!(photo_delegate_class()
            .instance_method(sel!(captureOutput:didFinishCaptureForResolvedSettings:error:))
            .is_some());
    }

    #[test]
    fn flash_modes_match_avfoundation_values() {
        assert_eq!(av_flash_mode(FlashMode::Off), 0);
        assert_eq!(av_flash_mode(FlashMode::Always), 1);
        assert_eq!(av_flash_mode(FlashMode::Auto), 2);
        assert_eq!(av_flash_mode(FlashMode::Torch), 0);
    }

    #[test]
    fn bounded_wait_reports_timeout() {
        let (_tx, rx) = mpsc::channel();
        let error = recv_bounded::<CapturedImage>(
            rx,
            Duration::from_millis(5),
            "Photo capture",
        )
        .unwrap_err();
        assert_eq!(error, "Photo capture timed out");
    }

    #[test]
    fn bounded_wait_returns_result() {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(CapturedImage {
            path: "/tmp/test.jpg".into(),
            width: 12,
            height: 34,
        }))
        .unwrap();
        let image = recv_bounded(rx, Duration::from_secs(1), "Photo capture").unwrap();
        assert_eq!(
            (image.path.as_str(), image.width, image.height),
            ("/tmp/test.jpg", 12u32, 34u32)
        );
    }

    #[test]
    fn bounded_wait_reports_closed_channel() {
        let (tx, rx) = mpsc::channel::<Result<RecordedVideo, String>>();
        drop(tx);
        let error = recv_bounded(rx, Duration::from_secs(1), "Video recording").unwrap_err();
        assert_eq!(error, "Video recording completion channel closed");
    }

    #[test]
    fn retained_session_survives_registry_removal() {
        unsafe {
            let session: *mut AnyObject = msg_send![class!(AVCaptureSession), new];
            let id = next_id();
            sessions().as_mut().unwrap().insert(
                id,
                SessionState {
                    operations: Arc::new(Mutex::new(())),
                    session,
                    device: std::ptr::null_mut(),
                    device_input: std::ptr::null_mut(),
                    photo_output: std::ptr::null_mut(),
                    video_output: std::ptr::null_mut(),
                    _enable_audio: false,
                    _audio_input: std::ptr::null_mut(),
                    flash_mode: FlashMode::Off,
                },
            );

            let retained = retain_session_ptr(id).unwrap();
            assert_eq!(retained, session);
            drop(sessions().as_mut().unwrap().remove(&id));

            let running: Bool = msg_send![retained, isRunning];
            let _ = running.as_bool();
            release(retained);
        }
    }
}
