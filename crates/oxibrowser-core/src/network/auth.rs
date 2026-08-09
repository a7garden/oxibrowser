//! HTTP authentication: `Basic` and `Digest` (RFC 7616) challenge handling.
//!
//! Used by [`super::HttpClient`] to retry a request with credentials after a
//! `401` + `WWW-Authenticate` challenge.

use base64::Engine;
use md5::{Digest, Md5};

/// Build an `Authorization` header value for a challenge, or `None` if the
/// scheme is unsupported / the challenge is malformed.
///
/// `challenge` is the raw `WWW-Authenticate` header value (e.g.
/// `Basic realm="x"` or `Digest realm="r", nonce="n", qop="auth"`).
pub fn build_authorization(
    challenge: &str,
    method: &str,
    uri: &str,
    username: &str,
    password: &str,
) -> Option<String> {
    let trimmed = challenge.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("basic") {
        Some(basic_auth(username, password))
    } else if lower.starts_with("digest") {
        digest_auth(trimmed, method, uri, username, password)
    } else {
        None
    }
}

/// `Basic <base64(user:password)>`.
fn basic_auth(username: &str, password: &str) -> String {
    let raw = format!("{username}:{password}");
    let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
    format!("Basic {encoded}")
}

/// Lowercase hex of an MD5 digest.
fn md5_hex(s: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(s.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parse `key=value` pairs from a challenge body (the text after the scheme
/// token). Comma-aware: commas inside a quoted-string are literal.
fn parse_digest_params(challenge: &str) -> std::collections::HashMap<String, String> {
    // Strip the leading scheme token ("Digest" / "Basic").
    let scheme_end = challenge
        .find(|c: char| c.is_whitespace())
        .unwrap_or(challenge.len());
    let body = challenge[scheme_end..].trim();

    let mut params = std::collections::HashMap::new();
    let mut in_quote = false;
    let mut token = String::new();
    for ch in body.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                token.push(ch);
            }
            ',' if !in_quote => {
                parse_one_token(&token, &mut params);
                token.clear();
            }
            _ => token.push(ch),
        }
    }
    if !token.trim().is_empty() {
        parse_one_token(&token, &mut params);
    }
    params
}

/// Parse a single `key=value` token (value may be quoted) into `params`.
fn parse_one_token(token: &str, params: &mut std::collections::HashMap<String, String>) {
    let token = token.trim();
    let Some((k, v)) = token.split_once('=') else {
        return;
    };
    let key = k.trim().to_ascii_lowercase();
    let val = v.trim().trim_matches('"').to_string();
    params.insert(key, val);
}

/// RFC 7616 Digest auth (MD5 + `qop=auth`). Returns the `Authorization:
/// Digest ...` header value, or `None` if required params are missing.
fn digest_auth(
    challenge: &str,
    method: &str,
    uri: &str,
    username: &str,
    password: &str,
) -> Option<String> {
    let p = parse_digest_params(challenge);
    let realm = p.get("realm")?;
    let nonce = p.get("nonce")?;
    let qop = p.get("qop").cloned();
    let algorithm = p.get("algorithm").map(|s| s.to_ascii_lowercase());
    let sess = algorithm.as_deref() == Some("md5-sess");

    let ha1 = md5_hex(&format!("{username}:{realm}:{password}"));
    let ha1 = if sess {
        let cnonce = cnonce();
        md5_hex(&format!("{ha1}:{nonce}:{cnonce}"))
    } else {
        ha1
    };
    let ha2 = md5_hex(&format!("{method}:{uri}"));

    let response = match qop {
        Some(q) if q.split(',').any(|t| t.trim() == "auth") => {
            let nc = "00000001";
            let cnonce = cnonce();
            let resp = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"));
            format!(
                "Digest username=\"{username}\", realm=\"{realm}\", nonce=\"{nonce}\", \
                 uri=\"{uri}\", qop=auth, nc={nc}, cnonce=\"{cnonce}\", response=\"{resp}\", \
                 algorithm=MD5"
            )
        }
        _ => {
            let resp = md5_hex(&format!("{ha1}:{nonce}:{ha2}"));
            format!(
                "Digest username=\"{username}\", realm=\"{realm}\", nonce=\"{nonce}\", \
                 uri=\"{uri}\", response=\"{resp}\", algorithm=MD5"
            )
        }
    };
    Some(response)
}

/// A deterministic-enough cnonce for a headless automation client. Digest
/// cnonces need only be unique/unguessable per-request; a monotonic counter
/// suffices here.
fn cnonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{n:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_authorization_header() {
        let h = build_authorization("Basic realm=\"x\"", "GET", "/", "Aladdin", "open sesame");
        assert_eq!(h.as_deref(), Some("Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="));
    }

    #[test]
    fn digest_authorization_without_qop() {
        let h = build_authorization(
            "Digest realm=\"testrealm@host.com\", nonce=\"abc123\"",
            "GET",
            "/dir/index.html",
            "Mufasa",
            "Circle Of Life",
        )
        .unwrap();
        assert!(h.starts_with("Digest username=\"Mufasa\""));
        assert!(h.contains("realm=\"testrealm@host.com\""));
        // No qop in the challenge → no qop/nc/cnonce in the response.
        assert!(!h.contains("qop="));
    }

    #[test]
    fn digest_authorization_with_qop_auth() {
        let h = build_authorization(
            "Digest realm=\"r\", nonce=\"n\", qop=\"auth\"",
            "GET",
            "/",
            "u",
            "p",
        )
        .unwrap();
        assert!(h.contains("qop=auth"));
        assert!(h.contains("nc=00000001"));
        assert!(h.contains("cnonce="));
    }

    #[test]
    fn unsupported_scheme_returns_none() {
        assert!(build_authorization("Bearer xyz", "GET", "/", "u", "p").is_none());
    }

    #[test]
    fn parse_digest_params_handles_quoted_values() {
        let p = parse_digest_params("Digest realm=\"o, k\", nonce=\"n\", qop=\"auth\"");
        assert_eq!(p.get("realm").unwrap(), "o, k");
        assert_eq!(p.get("nonce").unwrap(), "n");
        assert_eq!(p.get("qop").unwrap(), "auth");
    }
}
