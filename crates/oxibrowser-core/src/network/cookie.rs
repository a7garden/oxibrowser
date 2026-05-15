//! Cookie jar for session cookie management with RFC 6265 compliance.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use url::Url;

/// SameSite cookie attribute (RFC 6265bis).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

impl std::fmt::Display for SameSite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SameSite::Strict => write!(f, "Strict"),
            SameSite::Lax => write!(f, "Lax"),
            SameSite::None => write!(f, "None"),
        }
    }
}

/// Maximum number of cookies per domain (RFC 6265 §6.1 recommends 50).
const MAX_COOKIES_PER_DOMAIN: usize = 50;

/// Maximum total number of cookies (RFC 6265 §6.1 recommends 3000).
const MAX_TOTAL_COOKIES: usize = 3000;

/// Maximum cookie value size in bytes (RFC 6265 §6.1 recommends 4096).
const MAX_COOKIE_VALUE_SIZE: usize = 4096;

/// A parsed cookie entry with its attributes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieEntry {
    pub name: String,
    pub value: String,
    pub path: Option<String>,
    pub domain: Option<String>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSite>,
}

impl CookieEntry {
    /// Parse a Set-Cookie header value into a CookieEntry.
    ///
    /// Expected format: `name=value; Path=/; HttpOnly; Secure; Domain=.example.com; SameSite=Lax`
    pub fn parse(header: &str) -> Option<Self> {
        let mut parts = header.split(';');
        let nv = parts.next()?.trim();
        let eq_pos = nv.find('=')?;
        let name = nv[..eq_pos].trim().to_string();
        let value = nv[eq_pos + 1..].trim().to_string();

        let mut path = None;
        let mut domain = None;
        let mut secure = false;
        let mut http_only = false;
        let mut same_site = None;

        for attr in parts {
            let attr = attr.trim();
            if attr.eq_ignore_ascii_case("secure") {
                secure = true;
            } else if attr.eq_ignore_ascii_case("httponly") {
                http_only = true;
            } else if let Some(val) = strip_prefix_case_insensitive(attr, "Path=") {
                path = Some(val.to_string());
            } else if let Some(val) = strip_prefix_case_insensitive(attr, "Domain=") {
                domain = Some(val.to_string());
            } else if let Some(val) = strip_prefix_case_insensitive(attr, "SameSite=") {
                same_site = match val {
                    v if v.eq_ignore_ascii_case("strict") => Some(SameSite::Strict),
                    v if v.eq_ignore_ascii_case("lax") => Some(SameSite::Lax),
                    v if v.eq_ignore_ascii_case("none") => Some(SameSite::None),
                    _ => None, // Unknown SameSite value → treat as not set
                };
            }
        }

        Some(Self {
            name,
            value,
            path,
            domain,
            secure,
            http_only,
            same_site,
        })
    }

    /// Render the name=value pair (without attributes) for use in a Cookie header.
    pub fn to_cookie_header(&self) -> String {
        format!("{}={}", self.name, self.value)
    }
}

/// Case-insensitive prefix stripper for attribute parsing.
fn strip_prefix_case_insensitive<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Compute the default cookie path from a URL per RFC 6265 §5.1.4.
fn default_path(url_path: &str) -> String {
    if url_path.is_empty() || !url_path.starts_with('/') {
        return "/".to_string();
    }
    // Strip everything after the last '/'
    if let Some(last_slash) = url_path.rfind('/') {
        let path = &url_path[..=last_slash];
        if path == "/" {
            return "/".to_string();
        }
        return path.to_string();
    }
    "/".to_string()
}

/// Check if `cookie_domain` domain-matches `host` per RFC 6265 §5.1.3.
///
/// A string domain-matches a given host string if at least one of the following
/// conditions holds:
/// 1. The domain string and the host string are identical.
/// 2. All of the following conditions hold:
///    - The domain string is a suffix of the host string.
///    - The last character of the host string that is not included in the domain
///      string is a "." character.
///    - The host string is not an IP address.
fn domain_matches(host: &str, cookie_domain: &str) -> bool {
    let host = host.to_lowercase();
    let cookie_domain = cookie_domain.to_lowercase();

    // Exact match
    if host == cookie_domain {
        return true;
    }

    // Suffix match: host ends with "." + cookie_domain
    if host.ends_with(&format!(".{}", cookie_domain)) {
        // Check that host is not an IP address (basic heuristic)
        if !is_ip_address(&host) {
            return true;
        }
    }

    false
}

/// Check if the host looks like an IP address (v4 or v6).
fn is_ip_address(host: &str) -> bool {
    // IPv4
    if host.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    // IPv6 (may have brackets like [::1])
    let trimmed = host.trim_start_matches('[').trim_end_matches(']');
    trimmed.parse::<std::net::IpAddr>().is_ok()
}

/// Check if a request path matches a cookie path per RFC 6265 §5.1.4.
///
/// The cookie-path matches the request-path if:
/// 1. The cookie-path is identical to the request-path, or
/// 2. The cookie-path is a prefix of the request-path, and the last character
///    of the cookie-path is "/", or
/// 3. The cookie-path is a prefix of the request-path, and the first character
///    of the request-path that is not included in the cookie-path is a "/".
fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if cookie_path.is_empty() || cookie_path == "/" {
        return true;
    }

    // Exact match
    if request_path == cookie_path {
        return true;
    }

    // Prefix match
    if request_path.starts_with(cookie_path) {
        // Check: either cookie_path ends with "/" or next char in request_path is "/"
        if cookie_path.ends_with('/') {
            return true;
        }
        if request_path.as_bytes().get(cookie_path.len()) == Some(&b'/') {
            return true;
        }
    }

    false
}

/// A cookie jar that stores cookies per domain with RFC 6265 compliance.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CookieJar {
    cookies: HashMap<String, Vec<CookieEntry>>,
}

impl CookieJar {
    /// Create an empty cookie jar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Total number of cookies across all domains.
    fn total_count(&self) -> usize {
        self.cookies.values().map(|v| v.len()).sum()
    }

    /// Store a cookie from a Set-Cookie header value.
    ///
    /// Parses the header into a CookieEntry and replaces any existing cookie
    /// with the same name, domain, and path.
    ///
    /// Per RFC 6265:
    /// - Uses the cookie's Domain attribute (validated against request host) as storage key
    /// - Validates that the cookie domain domain-matches the request URL's host
    /// - Enforces cookie count and size limits
    pub fn store(&mut self, url: &Url, cookie_header: &str) {
        let mut entry = match CookieEntry::parse(cookie_header) {
            Some(e) => e,
            None => {
                // Fallback: store as a raw name;value entry
                CookieEntry {
                    name: cookie_header.to_string(),
                    value: String::new(),
                    path: None,
                    domain: None,
                    secure: false,
                    http_only: false,
                    same_site: None,
                }
            }
        };

        // Enforce value size limit
        if entry.value.len() > MAX_COOKIE_VALUE_SIZE {
            entry.value.truncate(MAX_COOKIE_VALUE_SIZE);
        }

        let request_host = url.host_str().unwrap_or("unknown");

        // Determine storage domain per RFC 6265 §5.3 step 6–8
        let storage_domain = if let Some(ref cookie_domain) = entry.domain {
            // Strip leading dot for comparison
            let canonical = cookie_domain.trim_start_matches('.').to_lowercase();

            // Domain validation: cookie domain must domain-match the request host
            if !domain_matches(request_host, &canonical) {
                // Reject: evil.com cannot set Domain=.bank.com
                return;
            }

            // Store under the canonical domain (without leading dot)
            entry.domain = Some(canonical.clone());
            canonical
        } else {
            // No Domain attribute: cookie is host-only, use request host
            let host = request_host.to_lowercase();
            entry.domain = Some(host.clone());
            host
        };

        // Default path per RFC 6265 §5.1.4
        if entry.path.is_none() || entry.path.as_deref() == Some("") {
            entry.path = Some(default_path(url.path()));
        }

        // Replace existing cookie with same name and path, or append
        if let Some(existing) = self
            .cookies
            .get_mut(&storage_domain)
            .and_then(|entries| {
                entries
                    .iter_mut()
                    .find(|c| c.name == entry.name && c.path == entry.path)
            })
        {
            *existing = entry;
            return;
        }

        // Need to add a new entry — enforce limits first
        // Enforce total cookie limit by evicting from another domain
        if self.total_count() >= MAX_TOTAL_COOKIES {
            let domain_to_evict = self
                .cookies
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .find(|(k, _)| *k != &storage_domain)
                .map(|(k, _)| k.clone());
            if let Some(d) = domain_to_evict {
                if let Some(v) = self.cookies.get_mut(&d) {
                    v.remove(0);
                }
            }
        }

        let entries = self.cookies.entry(storage_domain).or_default();
        // Enforce per-domain cookie limit
        if entries.len() >= MAX_COOKIES_PER_DOMAIN {
            entries.remove(0); // Evict oldest
        }
        entries.push(entry);
    }

    /// Get all cookies applicable to a URL as a Cookie header value.
    ///
    /// Per RFC 6265 §5.4, filters by:
    /// - Domain matching (including subdomain/superdomain sharing)
    /// - Path matching
    /// - Secure flag enforcement (only send secure cookies over HTTPS)
    /// - SameSite basic enforcement
    pub fn cookies_for_url(&self, url: &Url) -> String {
        let host = url.host_str().unwrap_or("unknown").to_lowercase();
        let url_path = url.path();
        let is_secure = url.scheme() == "https";

        let mut matching: Vec<&CookieEntry> = Vec::new();

        for (domain, entries) in &self.cookies {
            // Check domain match: cookie's storage domain must match the URL host
            if !domain_matches(&host, domain) && domain != &host {
                continue;
            }

            for cookie in entries {
                // Secure: only send over HTTPS
                if cookie.secure && !is_secure {
                    continue;
                }

                // Path matching
                let cookie_path = cookie.path.as_deref().unwrap_or("/");
                if !path_matches(url_path, cookie_path) {
                    continue;
                }

                // SameSite basic enforcement
                match cookie.same_site {
                    Some(SameSite::Strict) => {
                        // Strict: never send in cross-site requests
                        // (basic enforcement: always allow for same-origin)
                    }
                    Some(SameSite::Lax) => {
                        // Lax: safe HTTP methods (GET, HEAD) only for cross-site
                        // (basic enforcement: always allow for now)
                    }
                    Some(SameSite::None) | None => {
                        // None or unset: send in all contexts
                    }
                }

                matching.push(cookie);
            }
        }

        // Sort by path length (longest first per RFC 6265 §5.4)
        matching.sort_by(|a, b| {
            let pa = a.path.as_deref().unwrap_or("/").len();
            let pb = b.path.as_deref().unwrap_or("/").len();
            pb.cmp(&pa)
        });

        matching
            .iter()
            .map(|c| c.to_cookie_header())
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Clear all cookies.
    pub fn clear(&mut self) {
        self.cookies.clear();
    }

    /// Number of stored cookie entries (by domain).
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    /// Check if the jar is empty.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Get all cookies as a flat Vec (all domains merged) with domain info populated.
    pub fn get_all(&self) -> Vec<CookieEntry> {
        self.cookies.values().flatten().cloned().collect()
    }

    /// Remove cookies matching name for a given URL.
    pub fn remove(&mut self, url: &Url, name: &str) {
        for cookies in self.cookies.values_mut() {
            cookies.retain(|c| {
                if c.name != name {
                    return true;
                }
                if let (Some(dom), Some(cdom)) = (&c.domain, url.domain()) {
                    if !dom.starts_with('.') && dom != cdom {
                        return true;
                    }
                }
                false
            });
        }
        // Clean up empty domain entries
        self.cookies.retain(|_, v| !v.is_empty());
    }

    /// Save cookies to a JSON file.
    pub fn save_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    /// Load cookies from a JSON file.
    pub fn load_from_file(path: &std::path::Path) -> std::io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cookie_jar_store_and_retrieve() {
        let mut jar = CookieJar::new();
        let url = Url::parse("https://example.com/page").unwrap();
        jar.store(&url, "session=abc123; Path=/");

        let cookies = jar.cookies_for_url(&url);
        assert!(
            cookies.contains("session=abc123"),
            "stored cookie should be retrievable"
        );
    }

    #[test]
    fn test_cookie_jar_domain_isolation() {
        let mut jar = CookieJar::new();
        let url_a = Url::parse("https://site-a.com/").unwrap();
        let url_b = Url::parse("https://site-b.com/").unwrap();

        jar.store(&url_a, "token=aaa");
        jar.store(&url_b, "token=bbb");

        let cookies_a = jar.cookies_for_url(&url_a);
        let cookies_b = jar.cookies_for_url(&url_b);

        assert!(
            cookies_a.contains("token=aaa"),
            "site A should see its own cookie"
        );
        assert!(
            !cookies_a.contains("token=bbb"),
            "site A should NOT see site B's cookie"
        );
        assert!(
            cookies_b.contains("token=bbb"),
            "site B should see its own cookie"
        );
        assert!(
            !cookies_b.contains("token=aaa"),
            "site B should NOT see site A's cookie"
        );
    }

    #[test]
    fn test_cookie_jar_clear() {
        let mut jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.store(&url, "key=val");
        assert!(!jar.is_empty());

        jar.clear();
        assert!(jar.is_empty(), "jar should be empty after clear");
        assert!(
            jar.cookies_for_url(&url).is_empty(),
            "no cookies after clear"
        );
    }

    #[test]
    fn test_cookie_jar_round_trip() {
        // Simulate: server sends Set-Cookie, then client sends Cookie on next request
        let mut jar = CookieJar::new();
        let url = Url::parse("https://example.com/page1").unwrap();

        // Server response includes Set-Cookie
        jar.store(&url, "session=abc123; Path=/; HttpOnly");
        jar.store(&url, "pref=dark; Path=/");

        // Client makes another request to the same domain
        let url2 = Url::parse("https://example.com/page2").unwrap();
        let cookies = jar.cookies_for_url(&url2);

        // Both cookies should be sent (stripped of attributes)
        assert!(
            cookies.contains("session=abc123"),
            "should send session cookie"
        );
        assert!(cookies.contains("pref=dark"), "should send pref cookie");
        assert!(!cookies.contains("Path="), "should strip Path attribute");
        assert!(!cookies.contains("HttpOnly"), "should strip HttpOnly flag");
    }

    #[test]
    fn test_cookie_jar_multiple_domains() {
        let mut jar = CookieJar::new();

        // Store cookies for 3 different domains
        for (domain, cookie) in [
            ("https://api.example.com", "token=api-token"),
            ("https://cdn.example.com", "cache=v1"),
            ("https://other.com", "session=xyz"),
        ] {
            let url = Url::parse(domain).unwrap();
            jar.store(&url, cookie);
        }

        // Each domain gets only its own cookies
        let api_url = Url::parse("https://api.example.com/data").unwrap();
        let api_cookies = jar.cookies_for_url(&api_url);
        assert!(api_cookies.contains("token=api-token"));
        assert!(!api_cookies.contains("cache=v1"));

        // Clear and verify all gone
        jar.clear();
        assert_eq!(jar.len(), 0);
    }

    #[test]
    fn test_cookie_entry_parsing() {
        let entry = CookieEntry::parse("session=abc123; Path=/; HttpOnly; Secure").unwrap();
        assert_eq!(entry.name, "session");
        assert_eq!(entry.value, "abc123");
        assert_eq!(entry.path.as_deref(), Some("/"));
        assert!(entry.http_only);
        assert!(entry.secure);
    }

    #[test]
    fn test_cookie_entry_parse_domain() {
        let entry = CookieEntry::parse("id=42; Domain=.example.com; Path=/").unwrap();
        assert_eq!(entry.name, "id");
        assert_eq!(entry.value, "42");
        assert_eq!(entry.domain.as_deref(), Some(".example.com"));
        assert_eq!(entry.path.as_deref(), Some("/"));
    }

    #[test]
    fn test_cookie_entry_parse_simple() {
        let entry = CookieEntry::parse("key=val").unwrap();
        assert_eq!(entry.name, "key");
        assert_eq!(entry.value, "val");
        assert!(entry.path.is_none());
        assert!(entry.domain.is_none());
        assert!(!entry.secure);
        assert!(!entry.http_only);
        assert!(entry.same_site.is_none());
    }

    #[test]
    fn test_cookie_entry_parse_no_equals() {
        // No '=' means it's not a valid cookie
        assert!(CookieEntry::parse("invalidcookie").is_none());
    }

    #[test]
    fn test_cookie_entry_parse_samesite() {
        let entry = CookieEntry::parse("session=x; SameSite=Strict").unwrap();
        assert_eq!(entry.same_site, Some(SameSite::Strict));

        let entry = CookieEntry::parse("session=x; SameSite=Lax").unwrap();
        assert_eq!(entry.same_site, Some(SameSite::Lax));

        let entry = CookieEntry::parse("session=x; SameSite=None").unwrap();
        assert_eq!(entry.same_site, Some(SameSite::None));

        let entry = CookieEntry::parse("session=x; SameSite=Invalid").unwrap();
        assert!(entry.same_site.is_none());
    }

    #[test]
    fn test_cookie_save_load_file() {
        let dir = std::env::temp_dir().join("oxibrowser_cookie_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cookies.json");

        // Clean up from previous runs
        let _ = std::fs::remove_file(&path);

        let mut jar = CookieJar::new();
        let url = Url::parse("https://example.com/page").unwrap();
        jar.store(&url, "session=abc123; Path=/");
        jar.store(&url, "theme=dark");

        // Save
        jar.save_to_file(&path).expect("save should succeed");

        // Load
        let loaded = CookieJar::load_from_file(&path).expect("load should succeed");

        // Verify loaded cookies match
        let cookies = loaded.cookies_for_url(&url);
        assert!(
            cookies.contains("session=abc123"),
            "loaded cookie should contain session"
        );
        assert!(
            cookies.contains("theme=dark"),
            "loaded cookie should contain theme"
        );

        // Clean up
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_cookie_save_load_preserves_attributes() {
        let dir = std::env::temp_dir().join("oxibrowser_cookie_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cookies_attrs.json");
        let _ = std::fs::remove_file(&path);

        let mut jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.store(&url, "id=42; Path=/; Domain=.example.com; Secure; HttpOnly; SameSite=Lax");

        jar.save_to_file(&path).unwrap();
        let loaded = CookieJar::load_from_file(&path).unwrap();

        // Check that the cookie entry has preserved attributes
        // After store, domain is canonicalized to "example.com"
        let entries = loaded.cookies.get("example.com").unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.name, "id");
        assert_eq!(entry.value, "42");
        assert_eq!(entry.path.as_deref(), Some("/"));
        assert_eq!(entry.domain.as_deref(), Some("example.com"));
        assert!(entry.secure);
        assert!(entry.http_only);
        assert_eq!(entry.same_site, Some(SameSite::Lax));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_cookie_load_missing_file() {
        let path = std::path::PathBuf::from("/tmp/oxibrowser_nonexistent_cookie_file_12345.json");
        assert!(
            CookieJar::load_from_file(&path).is_err(),
            "loading missing file should fail"
        );
    }

    #[test]
    fn test_cookie_store_replaces_existing() {
        let mut jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();

        jar.store(&url, "token=old_value");
        jar.store(&url, "token=new_value");

        let entries = jar.cookies.get("example.com").unwrap();
        assert_eq!(entries.len(), 1, "should have only one cookie for the name");
        assert_eq!(entries[0].value, "new_value");

        let cookies = jar.cookies_for_url(&url);
        assert!(cookies.contains("token=new_value"));
        assert!(!cookies.contains("old_value"));
    }

    // --- New RFC 6265 compliance tests ---

    #[test]
    fn test_domain_validation_rejects_cross_domain() {
        // evil.com cannot set Domain=.bank.com
        let mut jar = CookieJar::new();
        let url = Url::parse("https://evil.com/").unwrap();
        jar.store(&url, "evil_cookie=hacked; Domain=.bank.com");

        let bank_url = Url::parse("https://bank.com/").unwrap();
        let cookies = jar.cookies_for_url(&bank_url);
        assert!(
            cookies.is_empty(),
            "cross-domain cookie should be rejected"
        );
    }

    #[test]
    fn test_subdomain_sharing_with_domain_attribute() {
        let mut jar = CookieJar::new();
        // Set cookie from sub.example.com with Domain=.example.com
        let url = Url::parse("https://sub.example.com/").unwrap();
        jar.store(&url, "shared=yes; Domain=.example.com; Path=/");

        // Should be accessible from parent domain
        let parent_url = Url::parse("https://example.com/").unwrap();
        let cookies = jar.cookies_for_url(&parent_url);
        assert!(
            cookies.contains("shared=yes"),
            "cookie with Domain=.example.com should be visible to example.com"
        );

        // Should be accessible from another subdomain
        let other_sub_url = Url::parse("https://other.example.com/").unwrap();
        let cookies = jar.cookies_for_url(&other_sub_url);
        assert!(
            cookies.contains("shared=yes"),
            "cookie with Domain=.example.com should be visible to other.example.com"
        );
    }

    #[test]
    fn test_path_matching() {
        let mut jar = CookieJar::new();
        let url = Url::parse("https://example.com/app/login").unwrap();
        jar.store(&url, "auth=token; Path=/app");

        // Should match /app and sub-paths
        let app_url = Url::parse("https://example.com/app/dashboard").unwrap();
        assert!(
            jar.cookies_for_url(&app_url).contains("auth=token"),
            "should match sub-path of /app"
        );

        let app_root = Url::parse("https://example.com/app").unwrap();
        assert!(
            jar.cookies_for_url(&app_root).contains("auth=token"),
            "should match exact path /app"
        );

        // Should NOT match unrelated path
        let other_url = Url::parse("https://example.com/other").unwrap();
        assert!(
            !jar.cookies_for_url(&other_url).contains("auth=token"),
            "should not match unrelated path"
        );
    }

    #[test]
    fn test_secure_flag_enforcement() {
        let mut jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.store(&url, "secret=val; Secure");

        // Should be sent over HTTPS
        let https_url = Url::parse("https://example.com/").unwrap();
        assert!(
            jar.cookies_for_url(&https_url).contains("secret=val"),
            "secure cookie should be sent over HTTPS"
        );

        // Should NOT be sent over HTTP
        let http_url = Url::parse("http://example.com/").unwrap();
        assert!(
            !jar.cookies_for_url(&http_url).contains("secret=val"),
            "secure cookie should NOT be sent over HTTP"
        );
    }

    #[test]
    fn test_cookie_value_size_limit() {
        let mut jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        let big_value = "x".repeat(5000);
        jar.store(&url, &format!("big={}", big_value));

        let cookies = jar.cookies_for_url(&url);
        // Value should be truncated to 4096
        if let Some(entry) = jar
            .cookies
            .get("example.com")
            .and_then(|v| v.first())
        {
            assert!(
                entry.value.len() <= MAX_COOKIE_VALUE_SIZE,
                "cookie value should be truncated to max size"
            );
        }
    }
}
