//! An image attached to a hosted run's first user message (ADR 0041).
//!
//! Hosted tier 1 only: no local runtime in the tier matrix takes image
//! input, and a model that cannot see an image must refuse it rather than
//! run without it. The bytes are held base64-encoded, because that is the
//! form both protocols carry and the form the context budget counts, and
//! nothing here can print them: `Debug` is written by hand and the request
//! body is traced through [`redact_images`].

use serde_json::Value;
use std::sync::Arc;

/// The largest image file accepted, in bytes: 5 MiB, Anthropic's per-image
/// limit, applied to both protocols so one bound holds for every hosted run.
/// Nothing is resized; a larger file is refused.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// One image, sniffed and encoded.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    media_type: &'static str,
    /// Shared: a request body is rendered more than once per turn while it
    /// is fitted to the context budget, and a copy of megabytes each time
    /// buys nothing.
    base64: Arc<str>,
}

impl std::fmt::Debug for Image {
    /// By hand, so a `{:?}` anywhere cannot put the image into a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("media_type", &self.media_type)
            .field("base64_len", &self.base64.len())
            .finish()
    }
}

/// Why a file is not an image this accepts. Never carries the bytes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    #[error(
        "it is {bytes} bytes, over the {}-byte (5 MiB) limit for an image; trellis does \
         not resize images",
        MAX_IMAGE_BYTES
    )]
    TooLarge { bytes: u64 },
    #[error("it is not a PNG, JPEG, GIF or WebP image by its first bytes")]
    NotAnImage,
}

impl Image {
    /// Sniffs the format from the magic bytes, not the file name, which
    /// says nothing about what the provider will be sent.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ImageError> {
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(ImageError::TooLarge {
                bytes: bytes.len() as u64,
            });
        }
        let media_type = sniff(bytes).ok_or(ImageError::NotAnImage)?;
        Ok(Self {
            media_type,
            base64: base64(bytes).into(),
        })
    }

    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    pub fn media_type(&self) -> &'static str {
        self.media_type
    }

    /// Length of the base64 text, the size a request carries and the size
    /// counted against `[model] context_bytes`.
    pub fn base64_len(&self) -> usize {
        self.base64.len()
    }

    pub(crate) fn base64(&self) -> &str {
        &self.base64
    }
}

fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Standard-alphabet base64 with padding (RFC 4648 §4). By hand rather than
/// a dependency for one encoder.
fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A copy of a request body with every image's data replaced by its length,
/// for the `request as sent` trace. The body sent is unchanged; this is what
/// a log may hold of it.
pub fn redact_images(body: &Value) -> Value {
    let mut v = body.clone();
    redact(&mut v);
    v
}

fn redact(v: &mut Value) {
    match v {
        Value::Object(m) => {
            // Anthropic: {"type":"image","source":{"type":"base64","data":..}}
            if m.get("type").and_then(Value::as_str) == Some("image")
                && let Some(Value::Object(src)) = m.get_mut("source")
                && let Some(Value::String(d)) = src.get_mut("data")
            {
                *d = format!("<{} bytes of base64 not logged>", d.len());
            }
            // OpenAI: {"type":"image_url","image_url":{"url":"data:..."}}
            if m.get("type").and_then(Value::as_str) == Some("image_url")
                && let Some(Value::Object(iu)) = m.get_mut("image_url")
                && let Some(Value::String(u)) = iu.get_mut("url")
                && u.starts_with("data:")
            {
                *u = format!("<{}-byte data URL not logged>", u.len());
            }
            m.values_mut().for_each(redact);
        }
        Value::Array(a) => a.iter_mut().for_each(redact),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o, "{i:?}");
        }
        assert_eq!(base64(&[0xFF, 0xFE, 0xFD]), "//79");
    }

    #[test]
    fn the_format_is_read_from_the_magic_bytes_and_nothing_else_is_taken() {
        let cases: [(&[u8], &str); 5] = [
            (b"\x89PNG\r\n\x1a\nrest", "image/png"),
            (&[0xFF, 0xD8, 0xFF, 0xE0, 0], "image/jpeg"),
            (b"GIF89a....", "image/gif"),
            (b"GIF87a....", "image/gif"),
            (b"RIFF\0\0\0\0WEBPVP8 ", "image/webp"),
        ];
        for (b, mt) in cases {
            assert_eq!(Image::from_bytes(b).unwrap().media_type(), mt);
        }
        // A key file named `.png` is still refused: the name is not read.
        // The header is built at run time so no key-shaped literal sits in
        // source (G5).
        let key_header = format!("-----BEGIN {} PRIVATE KEY-----", "OPENSSH");
        assert_eq!(
            Image::from_bytes(key_header.as_bytes()),
            Err(ImageError::NotAnImage)
        );
        assert_eq!(
            Image::from_bytes(b"RIFF\0\0\0\0WAVE"),
            Err(ImageError::NotAnImage)
        );
    }

    #[test]
    fn an_image_over_the_bound_is_refused_and_one_at_it_is_not() {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.resize(MAX_IMAGE_BYTES, 0);
        assert!(Image::from_bytes(&b).is_ok());
        b.push(0);
        assert_eq!(
            Image::from_bytes(&b),
            Err(ImageError::TooLarge {
                bytes: MAX_IMAGE_BYTES as u64 + 1
            })
        );
    }

    #[test]
    fn debug_and_the_redacted_body_hold_no_image_data() {
        let img = Image::from_bytes(b"GIF89a-secret-pixels-").unwrap();
        let data = img.base64().to_string();
        assert!(!format!("{img:?}").contains(&data));
        let body = json!({"messages":[{"content":[
            {"type":"image","source":{"type":"base64","media_type":"image/gif","data":data}},
            {"type":"image_url","image_url":{"url":format!("data:image/gif;base64,{data}")}},
            {"type":"text","text":"kept"}]}]});
        let r = redact_images(&body).to_string();
        assert!(!r.contains(&data), "{r}");
        assert!(r.contains("kept") && r.contains("not logged"), "{r}");
    }
}
