//! HTML character-encoding detection on the original response bytes.

use encoding_rs::{Encoding, UTF_8};

pub(crate) struct DecodedHtml {
    pub html: String,
    pub encoding: &'static str,
    pub had_errors: bool,
}

pub(crate) fn decode_html(content_type: Option<&str>, bytes: &[u8]) -> DecodedHtml {
    let detected = detect_charset(content_type.unwrap_or_default(), bytes);
    let encoding = detected
        .as_deref()
        .and_then(|label| Encoding::for_label(label.as_bytes()))
        .unwrap_or(UTF_8);
    let (html, actual_encoding, had_errors) = encoding.decode(bytes);
    DecodedHtml {
        html: html.into_owned(),
        encoding: actual_encoding.name(),
        had_errors,
    }
}

/// Detect an encoding before UTF-8 decoding destroys non-UTF-8 BOMs or text.
///
/// The ordered signals mirror Crawlberg's proven acquisition path: HTTP
/// `Content-Type`, a byte-order mark, then an ASCII-safe scan of the first 2 KiB
/// for an HTML `charset=` declaration.
fn detect_charset(content_type: &str, bytes: &[u8]) -> Option<String> {
    if let Some(position) = ascii_find_case_insensitive(content_type.as_bytes(), b"charset=") {
        let label = content_type[position + 8..]
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches(['\'', '"'])
            .to_ascii_lowercase();
        if !label.is_empty() {
            return Some(label);
        }
    }

    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Some("utf-8".to_string());
    }
    if bytes.starts_with(&[0xff, 0xfe]) {
        return Some("utf-16le".to_string());
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        return Some("utf-16be".to_string());
    }

    let head = &bytes[..bytes.len().min(2_048)];
    ascii_find_case_insensitive(head, b"charset=").and_then(|position| {
        let remainder = &head[position + 8..];
        let remainder = match remainder.first() {
            Some(b'\'' | b'"') => &remainder[1..],
            _ => remainder,
        };
        let end = remainder
            .iter()
            .position(|byte| {
                matches!(byte, b'\'' | b'"' | b'>' | b';') || byte.is_ascii_whitespace()
            })
            .unwrap_or(remainder.len());
        let label = String::from_utf8_lossy(&remainder[..end])
            .trim()
            .to_ascii_lowercase();
        (!label.is_empty()).then_some(label)
    })
}

fn ascii_find_case_insensitive(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

#[cfg(test)]
#[path = "charset_tests.rs"]
mod tests;
