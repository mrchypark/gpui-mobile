use super::{completion, ImagePickerOptions};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use std::ffi::c_void;
use std::path::Path;

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    fn CGImageSourceCreateWithURL(url: *mut AnyObject, options: *mut AnyObject) -> *const c_void;
    fn CGImageSourceCopyPropertiesAtIndex(
        source: *const c_void,
        index: usize,
        options: *mut AnyObject,
    ) -> *const c_void;
    fn CGImageSourceGetType(source: *const c_void) -> *mut AnyObject;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: *const c_void,
        index: usize,
        options: *mut AnyObject,
    ) -> *const c_void;
    fn CGImageDestinationCreateWithURL(
        url: *mut AnyObject,
        image_type: *mut AnyObject,
        count: usize,
        options: *mut AnyObject,
    ) -> *const c_void;
    fn CGImageDestinationAddImage(
        destination: *const c_void,
        image: *const c_void,
        properties: *mut AnyObject,
    );
    fn CGImageDestinationFinalize(destination: *const c_void) -> bool;
    static kCGImagePropertyPixelWidth: *mut AnyObject;
    static kCGImagePropertyPixelHeight: *mut AnyObject;
    static kCGImagePropertyOrientation: *mut AnyObject;
    static kCGImageSourceCreateThumbnailFromImageAlways: *mut AnyObject;
    static kCGImageSourceCreateThumbnailWithTransform: *mut AnyObject;
    static kCGImageSourceThumbnailMaxPixelSize: *mut AnyObject;
    static kCGImageDestinationLossyCompressionQuality: *mut AnyObject;
}
extern "C" {
    fn CFRelease(object: *const c_void);
}
struct Owned(*const c_void);
impl Owned {
    fn new(value: *const c_void) -> Result<Self, String> {
        if value.is_null() {
            Err("ImageIO could not decode or encode the selected image".into())
        } else {
            Ok(Self(value))
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0);
        }
    }
}

unsafe fn property(properties: &Owned, key: *mut AnyObject) -> u32 {
    let value: *mut AnyObject = msg_send![properties.0 as *mut AnyObject, objectForKey: key];
    if value.is_null() {
        0
    } else {
        msg_send![value, unsignedIntValue]
    }
}

unsafe fn transform(path: &str, options: &ImagePickerOptions) -> Result<String, String> {
    if options.max_width.is_none()
        && options.max_height.is_none()
        && options.image_quality.is_none()
    {
        return Ok(path.to_string());
    }
    let url: *mut AnyObject =
        msg_send![class!(NSURL), fileURLWithPath: crate::ios::util::nsstring(path)];
    let source = Owned::new(CGImageSourceCreateWithURL(url, std::ptr::null_mut()))?;
    let image_type = CGImageSourceGetType(source.0);
    let jpeg: Bool =
        msg_send![image_type, isEqualToString: crate::ios::util::nsstring("public.jpeg")];
    if !jpeg.as_bool() && options.max_width.is_none() && options.max_height.is_none() {
        return Ok(path.to_string()); // JPEG quality does not recompress PNG/HEIF/etc.
    }
    let properties = Owned::new(CGImageSourceCopyPropertiesAtIndex(
        source.0,
        0,
        std::ptr::null_mut(),
    ))?;
    let mut width = property(&properties, kCGImagePropertyPixelWidth);
    let mut height = property(&properties, kCGImagePropertyPixelHeight);
    if (5..=8).contains(&property(&properties, kCGImagePropertyOrientation)) {
        std::mem::swap(&mut width, &mut height);
    }
    let (width, height) =
        super::super::options::fit(width, height, options.max_width, options.max_height)?;
    let yes: *mut AnyObject = msg_send![class!(NSNumber), numberWithBool: Bool::YES];
    let pixels: *mut AnyObject =
        msg_send![class!(NSNumber), numberWithUnsignedInt: width.max(height)];
    let keys = [
        kCGImageSourceCreateThumbnailFromImageAlways,
        kCGImageSourceCreateThumbnailWithTransform,
        kCGImageSourceThumbnailMaxPixelSize,
    ];
    let values = [yes, yes, pixels];
    let settings: *mut AnyObject = msg_send![class!(NSDictionary), dictionaryWithObjects: values.as_ptr(), forKeys: keys.as_ptr(), count: keys.len()];
    let image = Owned::new(CGImageSourceCreateThumbnailAtIndex(source.0, 0, settings))?;
    let name = Path::new(path).file_stem().ok_or("Image has no filename")?;
    let directory = completion::private_import_directory()?;
    let destination_path =
        directory
            .join(name)
            .with_extension(if jpeg.as_bool() { "jpg" } else { "png" });
    let result = (|| {
        let destination_url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: crate::ios::util::nsstring(&destination_path.to_string_lossy())];
        let output_type = crate::ios::util::nsstring(if jpeg.as_bool() {
            "public.jpeg"
        } else {
            "public.png"
        });
        let destination = Owned::new(CGImageDestinationCreateWithURL(
            destination_url,
            output_type,
            1,
            std::ptr::null_mut(),
        ))?;
        let quality: *mut AnyObject = msg_send![class!(NSNumber), numberWithDouble: f64::from(options.image_quality.unwrap_or(100)) / 100.0];
        let compression: *mut AnyObject = msg_send![class!(NSDictionary), dictionaryWithObject: quality, forKey: kCGImageDestinationLossyCompressionQuality];
        CGImageDestinationAddImage(
            destination.0,
            image.0,
            if jpeg.as_bool() {
                compression
            } else {
                std::ptr::null_mut()
            },
        );
        if !CGImageDestinationFinalize(destination.0) {
            return Err("ImageIO failed to write resized image".into());
        }
        std::fs::remove_file(path).map_err(|e| format!("Remove intermediate import: {e}"))?;
        if let Some(parent) = Path::new(path).parent() {
            let _ = std::fs::remove_dir(parent);
        }
        Ok(destination_path.to_string_lossy().into_owned())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&destination_path);
        let _ = std::fs::remove_dir(&directory);
    }
    result
}

pub(super) fn process(
    paths: Vec<String>,
    options: &ImagePickerOptions,
) -> Result<Vec<String>, String> {
    let mut results = Vec::with_capacity(paths.len());
    for path in &paths {
        match unsafe { transform(path, options) } {
            Ok(path) => results.push(path),
            Err(error) => {
                for path in paths.iter().chain(results.iter()) {
                    let _ = std::fs::remove_file(path);
                    if let Some(parent) = Path::new(path).parent() {
                        let _ = std::fs::remove_dir(parent);
                    }
                }
                return Err(error);
            }
        }
    }
    Ok(results)
}
