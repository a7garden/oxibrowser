//! CORS policy (Fetch standard §3.2–3.3) and Referrer policy.
//!
//! Used by [`super::HttpClient`] to decide whether a cross-origin request
//! needs a preflight `OPTIONS`, to validate the preflight response, and to
//! compute the `Referer` header per the Chrome default
//! `strict-origin-when-cross-origin`.

/// CORS-safelisted methods (Fetch §3.3.5). A request using any other method is
/// not "simple" and needs a preflight.
pub fn is_safelisted_method(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "POST"
    )
}

/// CORS-safelisted request-header check, including the `Content-Type` value
/// restriction (Fetch §3.2.4.2).
fn is_safelisted_header(name: &str, value: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "accept" | "accept-language" | "content-language" | "range" => true,
        "content-type" => {
            let v = value.trim().to_ascii_lowercase();
            v.starts_with("application/x-www-form-urlencoded")
                || v.starts_with("multipart/form-data")
                || v.starts_with("text/plain")
                || v.is_empty()
        }
        _ => false,
    }
}

/// True if the request is not "simple" and therefore requires a CORS
/// preflight (`OPTIONS`) before the actual request.
pub fn requires_preflight(method: &str, headers: &[(String, String)]) -> bool {
    if !is_safelisted_method(method) {
        return true;
    }
    headers
        .iter()
        .any(|(name, value)| !is_safelisted_header(name, value))
}

/// Outcome of validating a preflight response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightResult {
    /// The actual request may proceed.
    Allowed,
    /// The server did not permit the request.
    Denied,
}

/// Validate a preflight response against the intended request.
///
/// `allow_origin` / `allow_methods` / `allow_headers` are the raw
/// `Access-Control-Allow-*` response header values (may be `None`).
/// `with_credentials` is true when the request carries cookies / an
/// `Authorization` header.
pub fn validate_preflight(
    method: &str,
    request_headers: &[(String, String)],
    allow_origin: Option<&str>,
    allow_methods: Option<&str>,
    allow_headers: Option<&str>,
    _allow_credentials: Option<&str>,
    with_credentials: bool,
) -> PreflightResult {
    // 1. Origin must be allowed.
    let origin_ok = match allow_origin {
        Some(v) => v.trim() == "*" || v.trim() == "null",
        None => false,
    };
    if !origin_ok {
        return PreflightResult::Denied;
    }
    // 2. Credentials + wildcard origin is forbidden (Fetch §3.2.1).
    if with_credentials && allow_origin.map(|v| v.trim()) == Some("*") {
        return PreflightResult::Denied;
    }
    // 3. Method must be listed (or wildcard).
    let method_ok = allow_methods.is_some_and(|v| {
        v.split(',').any(|m| {
            let m = m.trim();
            m == "*" || m.eq_ignore_ascii_case(method)
        })
    });
    if !method_ok {
        return PreflightResult::Denied;
    }
    // 4. Each non-safelisted request header must be listed (or wildcard).
    let allowed_set: Vec<&str> = allow_headers
        .map(|v| v.split(',').map(|s| s.trim()).collect())
        .unwrap_or_default();
    for (name, _value) in request_headers {
        if is_safelisted_header(name, "") {
            continue;
        }
        let allowed = allowed_set
            .iter()
            .any(|a| *a == "*" || a.eq_ignore_ascii_case(name));
        if !allowed {
            return PreflightResult::Denied;
        }
    }
    PreflightResult::Allowed
}

/// Compute the `Referer` header per Chrome's default policy
/// `strict-origin-when-cross-origin`.
///
/// - Same origin: the full URL (minus fragment).
/// - Cross origin, same scheme: the origin only.
/// - Downgrade (https → http): no Referer (`None`).
pub fn compute_referer(page_url: &url::Url, request_url: &url::Url) -> Option<String> {
    // Downgrade: never leak a Referer from a secure to an insecure origin.
    if page_url.scheme() == "https" && request_url.scheme() == "http" {
        return None;
    }
    if same_origin(page_url, request_url) {
        // Full URL without fragment.
        let mut s = page_url.as_str().to_string();
        if let Some(cut) = s.find('#') {
            s.truncate(cut);
        }
        Some(s)
    } else {
        // Cross-origin: origin only.
        Some(page_url.origin().ascii_serialization())
    }
}

/// Whether two URLs are same-origin (scheme + host + port match).
pub fn same_origin(a: &url::Url, b: &url::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && (a.port_or_known_default()) == (b.port_or_known_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safelisted_methods() {
        assert!(is_safelisted_method("GET"));
        assert!(is_safelisted_method("post"));
        assert!(!is_safelisted_method("PUT"));
        assert!(!is_safelisted_method("DELETE"));
    }

    #[test]
    fn preflight_required_for_put() {
        assert!(requires_preflight("PUT", &[]));
        assert!(!requires_preflight("GET", &[]));
    }

    #[test]
    fn preflight_required_for_custom_header() {
        assert!(!requires_preflight(
            "POST",
            &[("content-type".into(), "text/plain".into())]
        ));
        assert!(requires_preflight(
            "POST",
            &[("content-type".into(), "application/json".into())]
        ));
        assert!(requires_preflight(
            "GET",
            &[("x-custom".into(), "v".into())]
        ));
    }

    #[test]
    fn validate_preflight_wildcard_allows_anon() {
        assert_eq!(
            validate_preflight("PUT", &[], Some("*"), Some("*"), Some("*"), None, false),
            PreflightResult::Allowed
        );
    }

    #[test]
    fn validate_preflight_denies_missing_origin() {
        assert_eq!(
            validate_preflight("PUT", &[], None, Some("PUT"), None, None, false),
            PreflightResult::Denied
        );
    }

    #[test]
    fn validate_preflight_denies_method_not_listed() {
        assert_eq!(
            validate_preflight(
                "DELETE",
                &[],
                Some("*"),
                Some("GET, POST"),
                None,
                None,
                false
            ),
            PreflightResult::Denied
        );
    }

    #[test]
    fn referer_same_origin_full_url() {
        let page = url::Url::parse("https://example.com/a/b?x=1#frag").unwrap();
        let req = url::Url::parse("https://example.com/c").unwrap();
        assert_eq!(
            compute_referer(&page, &req).as_deref(),
            Some("https://example.com/a/b?x=1")
        );
    }

    #[test]
    fn referer_cross_origin_origin_only() {
        let page = url::Url::parse("https://example.com/a").unwrap();
        let req = url::Url::parse("https://api.other.com/c").unwrap();
        assert_eq!(
            compute_referer(&page, &req).as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn referer_downgrade_none() {
        let page = url::Url::parse("https://example.com/").unwrap();
        let req = url::Url::parse("http://example.com/").unwrap();
        assert_eq!(compute_referer(&page, &req), None);
    }
}
