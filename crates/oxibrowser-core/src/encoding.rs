//! Character encoding detection and decoding for HTML responses.
//!
//! Detects the encoding of raw HTML bytes using a priority chain:
//! 1. HTTP `Content-Type` header `charset` parameter
//! 2. BOM (Byte Order Mark)
//! 3. HTML `<meta charset="...">` or `<meta http-equiv="Content-Type">`
//! 4. Falls back to UTF-8
//!
//! Uses `encoding_rs` (Mozilla's WHATWG-standard encoding library) for decoding.

use encoding_rs::Encoding;

/// Decode raw HTML bytes into a UTF-8 string, auto-detecting the encoding.
///
/// The `content_type_header` is the value of the HTTP `Content-Type` response
/// header, if available. It is consulted first for a `charset` parameter.
pub fn decode_html(bytes: &[u8], content_type_header: Option<&str>) -> String {
    let encoding = detect_encoding(bytes, content_type_header);
    let (cow, _encoding_used, _had_errors) = encoding.decode(bytes);
    cow.into_owned()
}

/// Detect the encoding of raw HTML bytes.
///
/// Priority:
/// 1. `Content-Type` header charset
/// 2. BOM
/// 3. HTML `<meta>` charset (first 1024 bytes)
/// 4. UTF-8 fallback
fn detect_encoding(bytes: &[u8], content_type: Option<&str>) -> &'static Encoding {
    // 1. HTTP Content-Type header
    if let Some(ct) = content_type {
        if let Some(enc) = parse_content_type_charset(ct) {
            return enc;
        }
    }

    // 2. BOM
    if let Some(enc) = detect_bom(bytes) {
        return enc;
    }

    // 3. HTML <meta> charset (scan first 1024 bytes)
    if let Some(enc) = detect_meta_charset(bytes) {
        return enc;
    }

    // 4. UTF-8 fallback
    encoding_rs::UTF_8
}

/// Extract charset from a `Content-Type` header value.
///
/// Examples:
/// - `"text/html; charset=euc-kr"` → `EUC-KR`
/// - `"text/html; charset=utf-8"` → `UTF-8`
/// - `"text/html"` → `None`
fn parse_content_type_charset(ct: &str) -> Option<&'static Encoding> {
    let ct_lower = ct.to_ascii_lowercase();
    for part in ct_lower.split(';') {
        let part = part.trim();
        if let Some(charset_value) = part.strip_prefix("charset=") {
            let charset_value = charset_value.trim().trim_matches('"').trim_matches('\'');
            if let Some(enc) = Encoding::for_label(charset_value.as_bytes()) {
                return Some(enc);
            }
        }
    }
    None
}

/// Detect encoding from BOM (Byte Order Mark).
fn detect_bom(bytes: &[u8]) -> Option<&'static Encoding> {
    if bytes.len() < 2 {
        return None;
    }
    match bytes {
        // UTF-8 BOM
        [0xEF, 0xBB, 0xBF, ..] => Some(encoding_rs::UTF_8),
        // UTF-16 BE BOM
        [0xFE, 0xFF, ..] => Some(encoding_rs::UTF_16BE),
        // UTF-16 LE BOM
        [0xFF, 0xFE, ..] => Some(encoding_rs::UTF_16LE),
        _ => None,
    }
}

/// Detect encoding from HTML `<meta>` tags in the first 1024 bytes.
///
/// Looks for:
/// - `<meta charset="euc-kr">`
/// - `<meta http-equiv="Content-Type" content="text/html; charset=euc-kr">`
fn detect_meta_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    // Only scan the first 1024 bytes for performance
    let scan_len = bytes.len().min(1024);
    let scan = &bytes[..scan_len];

    // Convert to ASCII-lossy for case-insensitive matching
    // (non-ASCII bytes become '?', which won't match our patterns)
    let scan_ascii: Vec<u8> = scan.iter().map(|&b| b.to_ascii_lowercase()).collect();

    // Pattern 1: <meta charset="...">
    if let Some(enc) = find_meta_charset_attr(&scan_ascii) {
        return Some(enc);
    }

    // Pattern 2: <meta http-equiv="content-type" content="...;charset=...">
    if let Some(enc) = find_meta_equiv_charset(&scan_ascii) {
        return Some(enc);
    }

    None
}

/// Find `<meta charset="...">` in the lowercase ASCII scan buffer.
fn find_meta_charset_attr(scan: &[u8]) -> Option<&'static Encoding> {
    let pattern = b"charset=";
    let mut pos = 0;

    while let Some(start) = find_subslice(scan, pattern, pos) {
        let value_start = start + pattern.len();

        // Skip optional quote
        let (value_start, quote) = if value_start < scan.len() && scan[value_start] == b'"' {
            (value_start + 1, b'"')
        } else if value_start < scan.len() && scan[value_start] == b'\'' {
            (value_start + 1, b'\'')
        } else {
            (value_start, b' ')
        };

        // Find end of value (quote or whitespace or '>')
        let value_end = scan[value_start..]
            .iter()
            .position(|&b| b == quote || b == b' ' || b == b'>' || b == b'/')
            .map(|p| value_start + p)
            .unwrap_or(scan.len());

        if value_start < value_end {
            let charset_bytes = &scan[value_start..value_end];
            if let Some(enc) = Encoding::for_label(charset_bytes) {
                return Some(enc);
            }
        }

        pos = value_start;
    }

    None
}

/// Find `<meta http-equiv="content-type" content="...;charset=...">` in the scan buffer.
fn find_meta_equiv_charset(scan: &[u8]) -> Option<&'static Encoding> {
    // Find "content-type" after "http-equiv"
    let http_equiv = b"http-equiv";
    let content_type_val = b"content-type";

    let mut pos = 0;
    while let Some(he_start) = find_subslice(scan, http_equiv, pos) {
        // Check that content-type appears nearby
        let nearby_end = (he_start + 200).min(scan.len());
        let nearby = &scan[he_start..nearby_end];

        if find_subslice(nearby, content_type_val, 0).is_none() {
            pos = he_start + 1;
            continue;
        }

        // Now find "content=" after this
        let content_pattern = b"content=";
        let content_search_start = he_start;
        let content_search_end = (he_start + 300).min(scan.len());

        if let Some(c_start) = find_subslice(
            &scan[content_search_start..content_search_end],
            content_pattern,
            0,
        ) {
            let abs_start = content_search_start + c_start + content_pattern.len();

            // Skip quote
            let (val_start, quote) = if abs_start < scan.len() && scan[abs_start] == b'"' {
                (abs_start + 1, b'"')
            } else if abs_start < scan.len() && scan[abs_start] == b'\'' {
                (abs_start + 1, b'\'')
            } else {
                (abs_start, b' ')
            };

            // Find charset= within the content value
            let val_end = scan[val_start..]
                .iter()
                .position(|&b| b == quote || b == b'>')
                .map(|p| val_start + p)
                .unwrap_or(scan.len());

            let content_val = &scan[val_start..val_end];

            // Find charset= in the content value
            let charset_pattern = b"charset=";
            if let Some(cs_start) = find_subslice(content_val, charset_pattern, 0) {
                let cs_val_start = cs_start + charset_pattern.len();
                let cs_val_end = content_val[cs_val_start..]
                    .iter()
                    .position(|&b| b == b';' || b == b' ' || b == b'"' || b == b'\'')
                    .map(|p| cs_val_start + p)
                    .unwrap_or(content_val.len());

                if cs_val_start < cs_val_end {
                    let charset_bytes = &content_val[cs_val_start..cs_val_end];
                    if let Some(enc) = Encoding::for_label(charset_bytes) {
                        return Some(enc);
                    }
                }
            }
        }

        pos = he_start + 1;
    }

    None
}

/// Find a byte subslice starting from a given position.
fn find_subslice(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_content_type_charset ---

    #[test]
    fn test_charset_from_content_type_utf8() {
        let enc = parse_content_type_charset("text/html; charset=utf-8");
        assert_eq!(enc, Some(encoding_rs::UTF_8));
    }

    #[test]
    fn test_charset_from_content_type_euc_kr() {
        let enc = parse_content_type_charset("text/html; charset=euc-kr");
        assert_eq!(enc, Some(encoding_rs::EUC_KR));
    }

    #[test]
    fn test_charset_from_content_type_no_charset() {
        let enc = parse_content_type_charset("text/html");
        assert_eq!(enc, None);
    }

    #[test]
    fn test_charset_from_content_type_with_quotes() {
        let enc = parse_content_type_charset("text/html; charset=\"utf-8\"");
        assert_eq!(enc, Some(encoding_rs::UTF_8));
    }

    #[test]
    fn test_charset_from_content_type_case_insensitive() {
        let enc = parse_content_type_charset("Text/HTML; CharSet=EUC-KR");
        assert_eq!(enc, Some(encoding_rs::EUC_KR));
    }

    #[test]
    fn test_charset_from_content_type_shift_jis() {
        let enc = parse_content_type_charset("text/html; charset=shift_jis");
        assert!(enc.is_some());
    }

    // --- detect_bom ---

    #[test]
    fn test_bom_utf8() {
        let bytes: &[u8] = &[0xEF, 0xBB, 0xBF, b'h', b'i'];
        assert_eq!(detect_bom(bytes), Some(encoding_rs::UTF_8));
    }

    #[test]
    fn test_bom_utf16_be() {
        let bytes: &[u8] = &[0xFE, 0xFF, 0x00, b'h'];
        assert_eq!(detect_bom(bytes), Some(encoding_rs::UTF_16BE));
    }

    #[test]
    fn test_bom_utf16_le() {
        let bytes: &[u8] = &[0xFF, 0xFE, b'h', 0x00];
        assert_eq!(detect_bom(bytes), Some(encoding_rs::UTF_16LE));
    }

    #[test]
    fn test_bom_none() {
        let bytes: &[u8] = b"<html>";
        assert_eq!(detect_bom(bytes), None);
    }

    #[test]
    fn test_bom_too_short() {
        let bytes: &[u8] = &[0xEF];
        assert_eq!(detect_bom(bytes), None);
    }

    // --- detect_meta_charset ---

    #[test]
    fn test_meta_charset_tag() {
        let html = b"<html><head><meta charset=\"euc-kr\"></head><body>";
        let enc = detect_meta_charset(html);
        assert_eq!(enc, Some(encoding_rs::EUC_KR));
    }

    #[test]
    fn test_meta_charset_single_quote() {
        let html = b"<html><head><meta charset='utf-8'></head><body>";
        let enc = detect_meta_charset(html);
        assert_eq!(enc, Some(encoding_rs::UTF_8));
    }

    #[test]
    fn test_meta_charset_case_insensitive() {
        let html = b"<HTML><HEAD><META CHARSET=\"EUC-KR\"></HEAD><BODY>";
        let enc = detect_meta_charset(html);
        assert_eq!(enc, Some(encoding_rs::EUC_KR));
    }

    #[test]
    fn test_meta_equiv_content_type() {
        let html = b"<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=euc-kr\"></head>";
        let enc = detect_meta_charset(html);
        assert_eq!(enc, Some(encoding_rs::EUC_KR));
    }

    #[test]
    fn test_meta_equiv_case_insensitive() {
        let html = b"<HTML><HEAD><META HTTP-EQUIV=\"content-type\" CONTENT=\"text/html; charset=UTF-8\"></HEAD>";
        let enc = detect_meta_charset(html);
        assert_eq!(enc, Some(encoding_rs::UTF_8));
    }

    #[test]
    fn test_meta_charset_not_found() {
        let html = b"<html><head><title>Hello</title></head><body>";
        let enc = detect_meta_charset(html);
        assert_eq!(enc, None);
    }

    #[test]
    fn test_meta_charset_beyond_1024() {
        // charset in position > 1024 should not be found
        let padding = "x".repeat(1100);
        let html = format!("<html><head>{padding}<meta charset=\"euc-kr\"></head>");
        let enc = detect_meta_charset(html.as_bytes());
        assert_eq!(
            enc, None,
            "charset beyond 1024 bytes should not be detected"
        );
    }

    #[test]
    fn test_meta_charset_within_1024() {
        // charset in position < 1024 should be found even if doc is longer
        let after = "x".repeat(2000);
        let html = format!("<html><head><meta charset=\"euc-kr\">{after}</head>");
        let enc = detect_meta_charset(html.as_bytes());
        assert_eq!(enc, Some(encoding_rs::EUC_KR));
    }

    // --- decode_html (integration) ---

    #[test]
    fn test_decode_utf8_no_header() {
        let html = "안녕하세요".as_bytes();
        let result = decode_html(html, None);
        assert_eq!(result, "안녕하세요");
    }

    #[test]
    fn test_decode_euc_kr_from_header() {
        // "안녕" encoded in EUC-KR
        let euc_kr_bytes = encoding_rs::EUC_KR.encode("안녕").0;
        let ct = "text/html; charset=euc-kr";
        let result = decode_html(&euc_kr_bytes, Some(ct));
        assert_eq!(result, "안녕");
    }

    #[test]
    fn test_decode_euc_kr_from_meta() {
        // Build HTML with EUC-KR encoded body and a meta charset tag
        let korean = "한글 테스트";
        let euc_kr_bytes = encoding_rs::EUC_KR.encode(korean).0;
        let meta_tag = b"<meta charset=\"euc-kr\">";

        let mut html_bytes = Vec::new();
        html_bytes.extend_from_slice(b"<html><head>");
        html_bytes.extend_from_slice(meta_tag);
        html_bytes.extend_from_slice(b"</head><body>");
        html_bytes.extend_from_slice(&euc_kr_bytes);
        html_bytes.extend_from_slice(b"</body></html>");

        let result = decode_html(&html_bytes, Some("text/html"));
        assert!(
            result.contains(korean),
            "decoded should contain Korean text, got: {result:?}"
        );
    }

    #[test]
    fn test_decode_shift_jis_from_header() {
        // "こんにちは" encoded in Shift_JIS
        let shift_jis_bytes = encoding_rs::SHIFT_JIS.encode("こんにちは").0;
        let ct = "text/html; charset=shift_jis";
        let result = decode_html(&shift_jis_bytes, Some(ct));
        assert_eq!(result, "こんにちは");
    }

    #[test]
    fn test_decode_iso_8859_1() {
        // "café" encoded in ISO-8859-1 (é = 0xE9)
        let iso_bytes: &[u8] = b"caf\xe9";
        let ct = "text/html; charset=iso-8859-1";
        let result = decode_html(iso_bytes, Some(ct));
        assert_eq!(result, "café");
    }

    #[test]
    fn test_decode_content_type_takes_priority() {
        // UTF-8 bytes but Content-Type says ISO-8859-1 — header wins
        let utf8_bytes = "café".as_bytes();
        let ct = "text/html; charset=iso-8859-1";
        let result = decode_html(utf8_bytes, Some(ct));
        // The UTF-8 bytes for é (0xC3 0xA9) decoded as ISO-8859-1 produce "Ã©"
        assert_eq!(
            result, "cafÃ©",
            "header charset (ISO-8859-1) should be used to decode the UTF-8 bytes"
        );
    }

    #[test]
    fn test_decode_bom_takes_priority_over_no_header() {
        // UTF-8 BOM + valid UTF-8 content
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("안녕".as_bytes());
        let result = decode_html(&bytes, None);
        assert_eq!(result, "안녕");
    }

    #[test]
    fn test_decode_fallback_utf8() {
        // Plain ASCII with no hints — should decode as UTF-8
        let result = decode_html(b"<html><body>Hello</body></html>", None);
        assert_eq!(result, "<html><body>Hello</body></html>");
    }

    // --- find_subslice ---

    #[test]
    fn test_find_subslice_found() {
        assert_eq!(find_subslice(b"hello world", b"world", 0), Some(6));
    }

    #[test]
    fn test_find_sublice_not_found() {
        assert_eq!(find_subslice(b"hello world", b"xyz", 0), None);
    }

    #[test]
    fn test_find_sublice_with_offset() {
        assert_eq!(find_subslice(b"abcabc", b"abc", 1), Some(3));
    }
}
