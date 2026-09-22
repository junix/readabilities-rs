use super::*;

#[test]
fn content_type_precedes_meta_and_decodes_windows_1252() {
    let bytes = b"<meta charset=utf-8><p>caf\xe9</p>";
    let decoded = decode_html(Some("text/html; charset=iso-8859-1"), bytes);
    assert_eq!(decoded.encoding, "windows-1252");
    assert!(decoded.html.contains("caf\u{e9}"), "{}", decoded.html);
    assert!(!decoded.had_errors);
}

#[test]
fn detects_meta_charset_and_utf16_bom_from_raw_bytes() {
    assert_eq!(
        detect_charset("text/html", br#"<meta charset="shift_jis">"#).as_deref(),
        Some("shift_jis")
    );
    assert_eq!(
        detect_charset("text/html", &[0xff, 0xfe, b'h', 0]).as_deref(),
        Some("utf-16le")
    );
}

#[test]
fn falls_back_to_lossy_utf8_without_a_supported_signal() {
    let decoded = decode_html(Some("text/html; charset=not-real"), b"bad \xff utf8");
    assert_eq!(decoded.encoding, "UTF-8");
    assert!(decoded.had_errors);
    assert!(decoded.html.contains('\u{fffd}'));
}
