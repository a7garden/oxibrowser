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
