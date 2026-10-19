use super::super::jni as bridge;
use anyhow::{anyhow, Result};
use gpui::ClipboardItem;
use jni::objects::{JByteArray, JValue};
#[path = "../clipboard_wire.rs"]
mod wire;
const HELPER: &str = "dev.gpui.mobile.GpuiClipboard";
pub(super) fn write(item: &ClipboardItem) -> Result<()> {
    let encoded = wire::encode(item)?;
    bridge::with_env(|env| {
        let result = (|| -> Result<()> {
            let activity = bridge::activity(env).map_err(anyhow::Error::msg)?;
            let class = bridge::find_app_class(env, HELPER).map_err(anyhow::Error::msg)?;
            let data = env.byte_array_from_slice(&encoded)?;
            env.call_static_method(
                &class,
                jni::jni_str!("write"),
                jni::jni_sig!("(Landroid/app/Activity;[B)V"),
                &[JValue::Object(&activity), JValue::Object(&data)],
            )?;
            Ok(())
        })();
        if result.is_err() {
            let _ = env.exception_clear();
        }
        result.map_err(|error| error.to_string())
    })
    .map_err(anyhow::Error::msg)
}
pub(super) fn read() -> Result<Option<ClipboardItem>> {
    let bytes = bridge::with_env(|env| {
        let mut result = (|| -> Result<Option<Vec<u8>>> {
            let activity = bridge::activity(env).map_err(anyhow::Error::msg)?;
            let class = bridge::find_app_class(env, HELPER).map_err(anyhow::Error::msg)?;
            let data = env
                .call_static_method(
                    &class,
                    jni::jni_str!("read"),
                    jni::jni_sig!("(Landroid/app/Activity;)[B"),
                    &[JValue::Object(&activity)],
                )?
                .l()?;
            if data.is_null() {
                return Ok(None);
            }
            let data = unsafe { JByteArray::from_raw(env, data.into_raw()) };
            Ok(Some(env.convert_byte_array(&data)?))
        })();
        if result.is_err() {
            let exception = env.exception_occurred();
            let _ = env.exception_clear();
            if let Some(exception) = exception {
                if env
                    .is_instance_of(&exception, jni::jni_str!("java/lang/SecurityException"))
                    .unwrap_or(false)
                {
                    result = Err(anyhow!(gpui::ClipboardReadError::Denied(
                        "Android denied clipboard/provider access".into()
                    )));
                }
            }
        }
        Ok::<_, String>(result)
    })
    .map_err(|error| {
        log::warn!("Clipboard unavailable: {error}");
        anyhow!(gpui::ClipboardReadError::Unavailable)
    })??;
    bytes.map(|bytes| wire::decode(&bytes)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires foreground Android Activity and native ClipboardManager; replaces clipboard"]
    fn native_text_metadata_empty_and_encoded_image_round_trip() {
        let mut item =
            ClipboardItem::new_string_with_metadata("a\0한😀".into(), "metadata\0".into());
        item.entries
            .push(gpui::ClipboardEntry::String(gpui::ClipboardString::new(
                String::new(),
            )));
        // Real 1x1 PNG. The clipboard must retain its encoded bytes, not transcode.
        let png = [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15,
            0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ];
        item.entries
            .push(gpui::Image::from_bytes(gpui::ImageFormat::Png, png.to_vec()).into());
        let result = (|| -> Result<()> {
            write(&item)?;
            if read()? != Some(item) {
                anyhow::bail!("Native clipboard round trip mismatch");
            }
            Ok(())
        })();
        let cleanup = write(&ClipboardItem {
            entries: Vec::new(),
        });
        cleanup.unwrap();
        result.unwrap();
    }
}
