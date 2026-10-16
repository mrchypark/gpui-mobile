//! Lossless native clipboard transport (big-endian lengths, UTF-8, raw image bytes).
use anyhow::{Result, anyhow, bail};
use gpui::{ClipboardEntry, ClipboardItem, ClipboardString, ExternalPaths, Image, ImageFormat};

pub(super) const FORMATS: &[(ImageFormat, &str, &str)] = &[
    (ImageFormat::Png, "image/png", "public.png"),
    (ImageFormat::Jpeg, "image/jpeg", "public.jpeg"),
    (ImageFormat::Webp, "image/webp", "org.webmproject.webp"),
    (ImageFormat::Gif, "image/gif", "com.compuserve.gif"),
    (ImageFormat::Svg, "image/svg+xml", "public.svg-image"),
    (ImageFormat::Bmp, "image/bmp", "com.microsoft.bmp"),
    (ImageFormat::Tiff, "image/tiff", "public.tiff"),
    (ImageFormat::Ico, "image/x-icon", "com.microsoft.ico"),
    (ImageFormat::Pnm, "image/x-portable-anymap", "public.pbm"),
];
pub(super) fn format(mime: &str) -> Result<ImageFormat> {
    let mime = match mime {
        "image/x-ms-bmp" => "image/bmp",
        "image/vnd.microsoft.icon" => "image/x-icon",
        other => other,
    };
    FORMATS
        .iter()
        .find(|(_, value, _)| *value == mime)
        .map(|(format, _, _)| *format)
        .ok_or_else(|| anyhow!("Unsupported clipboard image type: {mime}"))
}
pub(super) fn field(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    out.extend_from_slice(&i32::try_from(bytes.len())?.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}
pub(super) fn encode(item: &ClipboardItem) -> Result<Vec<u8>> {
    let count: usize = item
        .entries
        .iter()
        .map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => paths.0.len(),
            _ => 1,
        })
        .sum();
    let mut out = i32::try_from(count)?.to_be_bytes().to_vec();
    for entry in &item.entries {
        match entry {
            ClipboardEntry::String(value) => {
                out.push(0);
                field(&mut out, value.text.as_bytes())?;
                out.push(u8::from(value.metadata.is_some()));
                if let Some(metadata) = &value.metadata {
                    field(&mut out, metadata.as_bytes())?;
                }
            }
            ClipboardEntry::Image(image) => {
                out.push(1);
                let (_, mime, _) = FORMATS
                    .iter()
                    .find(|(format, _, _)| *format == image.format)
                    .ok_or_else(|| anyhow!("Unsupported image format"))?;
                field(&mut out, mime.as_bytes())?;
                field(&mut out, &image.bytes)?;
            }
            ClipboardEntry::ExternalPaths(paths) => {
                for path in &paths.0 {
                    out.push(2);
                    field(
                        &mut out,
                        path.to_str()
                            .ok_or_else(|| anyhow!("Clipboard path is not UTF-8"))?
                            .as_bytes(),
                    )?;
                }
            }
        }
    }
    Ok(out)
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let data = self
            .0
            .get(..len)
            .ok_or_else(|| anyhow!("Truncated clipboard payload"))?;
        self.0 = &self.0[len..];
        Ok(data)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn length(&mut self) -> Result<usize> {
        Ok(usize::try_from(i32::from_be_bytes(
            self.take(4)?.try_into()?,
        ))?)
    }
    fn field(&mut self) -> Result<&'a [u8]> {
        let len = self.length()?;
        self.take(len)
    }
    fn text(&mut self) -> Result<String> {
        Ok(std::str::from_utf8(self.field()?)?.to_owned())
    }
}
pub(super) fn decode(bytes: &[u8]) -> Result<ClipboardItem> {
    let mut reader = Reader(bytes);
    let count = reader.length()?;
    if count > reader.0.len() {
        bail!("Invalid clipboard entry count");
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(match reader.byte()? {
            0 => {
                let text = reader.text()?;
                let metadata = match reader.byte()? {
                    0 => None,
                    1 => Some(reader.text()?),
                    _ => bail!("Invalid clipboard metadata flag"),
                };
                ClipboardEntry::String(ClipboardString { text, metadata })
            }
            1 => {
                let format = format(&reader.text()?)?;
                ClipboardEntry::Image(Image::from_bytes(format, reader.field()?.to_vec()))
            }
            2 => ClipboardEntry::ExternalPaths(ExternalPaths(
                std::iter::once(std::path::PathBuf::from(reader.text()?)).collect(),
            )),
            _ => bail!("Invalid clipboard entry kind"),
        });
    }
    if !reader.0.is_empty() {
        bail!("Trailing clipboard payload");
    }
    Ok(ClipboardItem { entries })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_metadata_empty_text_and_order_round_trip() {
        let mut item =
            ClipboardItem::new_string_with_metadata("\0한😀".into(), "\0metadata".into());
        item.entries
            .push(ClipboardEntry::String(ClipboardString::new(String::new())));
        for (format, _, _) in FORMATS {
            item.entries
                .push(Image::from_bytes(*format, vec![0, 128, 255]).into());
        }
        item.entries
            .push(ClipboardEntry::ExternalPaths(ExternalPaths(
                std::iter::once("/private/file space".into()).collect(),
            )));
        assert_eq!(decode(&encode(&item).unwrap()).unwrap(), item);
        assert_eq!(
            decode(&encode(&ClipboardItem { entries: vec![] }).unwrap())
                .unwrap()
                .entries
                .len(),
            0
        );
    }
    #[test]
    fn corrupt_transport_never_returns_partial_success() {
        let bytes = encode(&ClipboardItem::new_string("abc".into())).unwrap();
        for end in 0..bytes.len() {
            assert!(decode(&bytes[..end]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        assert!(decode(&[255, 255, 255, 255]).is_err());
        assert!(format("image/unknown").is_err());
    }
}
