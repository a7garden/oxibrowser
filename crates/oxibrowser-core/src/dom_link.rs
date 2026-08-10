//! External `<link rel=stylesheet>` plumbing — fetch, inline, strip.
//!
//! Used by both [`crate::frame::Frame::from_html`] (panics guard) and
//! [`crate::session::Session::inject_dom_snapshot`] (full fetch + inline
//! path). Implementation favors simple regex over hand-rolled tag parsing —
//! the doc strings here are minimal HTML, and regex tooling is already in the
//! dependency graph via the `regex` crate (used elsewhere in the core).

use regex::Regex;
use std::sync::LazyLock;

/// All link-tag regexes share one initialization site so the runtime cost is
/// paid exactly once.
struct Compiled {
    link_tag: Regex,
    dq_attr: Regex,
    bare_attr: Regex,
}

static COMPILED: LazyLock<Compiled> = LazyLock::new(|| Compiled {
    // (?is) = case-insensitive, `.` matches newlines.
    link_tag: Regex::new(r#"(?is)<link\b([^>]*)>"#).unwrap(),
    dq_attr: Regex::new(r#"(?is)\b([a-zA-Z_:][a-zA-Z0-9_.:-]*)\s*=\s*"([^"]*)""#).unwrap(),
    bare_attr: Regex::new(r#"(?is)\b([a-zA-Z_:][a-zA-Z0-9_.:-]*)\s*=\s*([^>\s]+)"#).unwrap(),
});

/// Find every `<link rel=stylesheet href=…>` and return the `href` values in
/// document order. Both `<link rel="stylesheet" href="/x.css">` and
/// `<link rel=stylesheet href=/x.css>` are recognized; quoted values are
/// unquoted.
pub fn external_stylesheet_links(html: &str) -> Vec<String> {
    let c = &*COMPILED;
    let mut out = Vec::new();
    for cap in c.link_tag.captures_iter(html) {
        let Some(attrs) = cap.get(1) else { continue };
        let attrs_str = attrs.as_str();
        let mut rel_is_stylesheet = false;
        let mut href: Option<String> = None;
        // Try both quoted and bare forms for every attribute; a real-world
        // link tag may use either style for `rel` and `href`.
        for dq in c.dq_attr.captures_iter(attrs_str) {
            let name = dq.get(1).unwrap().as_str();
            let value = dq.get(2).unwrap().as_str();
            if name.eq_ignore_ascii_case("rel") && value.eq_ignore_ascii_case("stylesheet") {
                rel_is_stylesheet = true;
            } else if name.eq_ignore_ascii_case("href") && !value.is_empty() {
                href = Some(value.to_string());
            }
        }
        for bare in c.bare_attr.captures_iter(attrs_str) {
            let name = bare.get(1).unwrap().as_str();
            let value = bare.get(2).unwrap().as_str();
            if name.eq_ignore_ascii_case("rel") && value.eq_ignore_ascii_case("stylesheet") {
                rel_is_stylesheet = true;
            } else if name.eq_ignore_ascii_case("href") && !value.is_empty() && href.is_none() {
                // Bare match (unquoted); prefer the quoted value if already seen.
                href = Some(value.to_string());
            }
        }
        if rel_is_stylesheet && let Some(h) = href {
            out.push(h);
        }
    }
    out
}

/// Drop every `<link rel=stylesheet …>` tag from `html`. A `<link rel=…>` that
/// is *not* a stylesheet (e.g. `rel="icon"`) is preserved.
pub fn strip_stylesheet_links(html: &str) -> String {
    let c = &*COMPILED;
    let result = c.link_tag.replace_all(html, |cap: &regex::Captures<'_>| {
        let attrs_str = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let mut rel_is_stylesheet = false;
        let mut href: Option<&str> = None;
        for dq in c.dq_attr.captures_iter(attrs_str) {
            let name = dq.get(1).unwrap().as_str();
            let value = dq.get(2).unwrap().as_str();
            if name.eq_ignore_ascii_case("rel") && value.eq_ignore_ascii_case("stylesheet") {
                rel_is_stylesheet = true;
            } else if name.eq_ignore_ascii_case("href") && !value.is_empty() {
                href = Some(value);
            }
        }
        for bare in c.bare_attr.captures_iter(attrs_str) {
            let name = bare.get(1).unwrap().as_str();
            let value = bare.get(2).unwrap().as_str();
            if name.eq_ignore_ascii_case("rel") && value.eq_ignore_ascii_case("stylesheet") {
                rel_is_stylesheet = true;
            } else if name.eq_ignore_ascii_case("href") && !value.is_empty() && href.is_none() {
                href = Some(value);
            }
        }
        if rel_is_stylesheet && href.is_some() {
            String::new()
        } else {
            cap[0].to_string()
        }
    });
    result.into_owned()
}

/// Insert a `<style>…</style>` block immediately before `</head>` if present,
/// else before `</body>` if present, else at the end of `html`.
pub fn inject_inline_style(html: &str, css: &str) -> String {
    let block = format!("<style>{css}</style>");
    let lc = html.to_ascii_lowercase();
    if let Some(idx) = lc.find("</head>") {
        let mut out = String::with_capacity(html.len() + block.len());
        out.push_str(&html[..idx]);
        out.push_str(&block);
        out.push_str(&html[idx..]);
        return out;
    }
    if let Some(idx) = lc.find("</body>") {
        let mut out = String::with_capacity(html.len() + block.len());
        out.push_str(&html[..idx]);
        out.push_str(&block);
        out.push_str(&html[idx..]);
        return out;
    }
    let mut out = String::with_capacity(html.len() + block.len());
    out.push_str(html);
    out.push_str(&block);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_quoted_stylesheet_link() {
        let html = r#"<html><head><link rel="stylesheet" href="/foo.css"></head><body/></html>"#;
        let r = external_stylesheet_links(html);
        assert_eq!(r, vec!["/foo.css".to_string()]);
    }

    #[test]
    fn finds_bare_stylesheet_link() {
        let html = r#"<link rel=stylesheet href="/bar.css">"#;
        let r = external_stylesheet_links(html);
        assert_eq!(r, vec!["/bar.css".to_string()]);
    }

    #[test]
    fn ignores_non_stylesheet_rel() {
        let html = r#"<link rel="icon" href="/i.png">"#;
        let r = external_stylesheet_links(html);
        assert!(r.is_empty());
    }
    #[test]
    fn finds_real_world_mock_html() {
        let html = "<!DOCTYPE html><html><head>\n  <link rel=\"stylesheet\" href=\"/style.css\">\n</head><body>\n  <p id=\"t\" class=\"green\">probe</p>\n</body></html>";
        let r = external_stylesheet_links(html);
        assert_eq!(r, vec!["/style.css".to_string()]);
    }
    #[test]
    fn strip_removes_stylesheet_link() {
        let html = r#"<html><head><link rel="stylesheet" href="/foo.css"></head><body/></html>"#;
        let s = strip_stylesheet_links(html);
        assert!(!s.contains("stylesheet"));
        assert!(!s.contains("foo.css"));
    }
}
