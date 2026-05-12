//! Cookie jar for session cookie management.

use std::collections::HashMap;
use url::Url;

/// A simple cookie jar that stores cookies per domain.
#[derive(Debug, Default)]
pub struct CookieJar {
    cookies: HashMap<String, Vec<String>>,
}

impl CookieJar {
    /// Create an empty cookie jar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Store a cookie from a Set-Cookie header value.
    pub fn store(&mut self, url: &Url, cookie_header: &str) {
        let domain = url
            .host_str()
            .unwrap_or("unknown")
            .to_string();
        self.cookies
            .entry(domain)
            .or_default()
            .push(cookie_header.to_string());
    }

    /// Get all cookies for a URL as a Cookie header value.
    pub fn cookies_for_url(&self, url: &Url) -> String {
        let domain = url.host_str().unwrap_or("unknown");
        self.cookies
            .get(domain)
            .map(|cs| {
                cs.iter()
                    .filter_map(|c| c.split(';').next())
                    .map(|s| s.trim())
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default()
    }

    /// Clear all cookies.
    pub fn clear(&mut self) {
        self.cookies.clear();
    }

    /// Number of stored cookie entries.
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    /// Check if the jar is empty.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
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
        assert!(cookies.contains("session=abc123"), "stored cookie should be retrievable");
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

        assert!(cookies_a.contains("token=aaa"), "site A should see its own cookie");
        assert!(!cookies_a.contains("token=bbb"), "site A should NOT see site B's cookie");
        assert!(cookies_b.contains("token=bbb"), "site B should see its own cookie");
        assert!(!cookies_b.contains("token=aaa"), "site B should NOT see site A's cookie");
    }

    #[test]
    fn test_cookie_jar_clear() {
        let mut jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.store(&url, "key=val");
        assert!(!jar.is_empty());

        jar.clear();
        assert!(jar.is_empty(), "jar should be empty after clear");
        assert!(jar.cookies_for_url(&url).is_empty(), "no cookies after clear");
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
        assert!(cookies.contains("session=abc123"), "should send session cookie");
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
}
