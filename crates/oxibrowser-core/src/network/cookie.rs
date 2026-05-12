//! Cookie jar for session cookie management.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use url::Url;

/// A parsed cookie entry with its attributes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieEntry {
    pub name: String,
    pub value: String,
    pub path: Option<String>,
    pub domain: Option<String>,
    pub secure: bool,
    pub http_only: bool,
}

impl CookieEntry {
    /// Parse a Set-Cookie header value into a CookieEntry.
    ///
    /// Expected format: `name=value; Path=/; HttpOnly; Secure; Domain=.example.com`
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

        for attr in parts {
            let attr = attr.trim();
            if attr.eq_ignore_ascii_case("secure") {
                secure = true;
            } else if attr.eq_ignore_ascii_case("httponly") {
                http_only = true;
            } else if let Some(val) = attr
                .strip_prefix("Path=")
                .or_else(|| attr.strip_prefix("path="))
            {
                path = Some(val.to_string());
            } else if let Some(val) = attr
                .strip_prefix("Domain=")
                .or_else(|| attr.strip_prefix("domain="))
            {
                domain = Some(val.to_string());
            }
        }

        Some(Self {
            name,
            value,
            path,
            domain,
            secure,
            http_only,
        })
    }

    /// Render the name=value pair (without attributes) for use in a Cookie header.
    pub fn to_cookie_header(&self) -> String {
        format!("{}={}", self.name, self.value)
    }
}

/// A cookie jar that stores cookies per domain.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CookieJar {
    cookies: HashMap<String, Vec<CookieEntry>>,
}

impl CookieJar {
    /// Create an empty cookie jar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Store a cookie from a Set-Cookie header value.
    ///
    /// Parses the header into a CookieEntry and replaces any existing cookie
    /// with the same name for that domain.
    pub fn store(&mut self, url: &Url, cookie_header: &str) {
        let domain = url
            .host_str()
            .unwrap_or("unknown")
            .to_string();

        let entry = match CookieEntry::parse(cookie_header) {
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
                }
            }
        };

        let entries = self.cookies.entry(domain).or_default();
        // Replace existing cookie with same name, or append
        if let Some(existing) = entries.iter_mut().find(|c| c.name == entry.name) {
            *existing = entry;
        } else {
            entries.push(entry);
        }
    }

    /// Get all cookies for a URL as a Cookie header value.
    pub fn cookies_for_url(&self, url: &Url) -> String {
        let domain = url.host_str().unwrap_or("unknown");
        self.cookies
            .get(domain)
            .map(|cs| {
                cs.iter()
                    .map(|c| c.to_cookie_header())
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
        assert!(cookies.contains("session=abc123"), "should send session cookie");
        assert!(cookies.contains("pref=dark"), "should send pref cookie");
        assert!(!cookies.contains("Path="), "should strip Path attribute");
        assert!(
            !cookies.contains("HttpOnly"),
            "should strip HttpOnly flag"
        );
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
    }

    #[test]
    fn test_cookie_entry_parse_no_equals() {
        // No '=' means it's not a valid cookie
        assert!(CookieEntry::parse("invalidcookie").is_none());
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
        jar.store(&url, "id=42; Path=/; Domain=.example.com; Secure; HttpOnly");

        jar.save_to_file(&path).unwrap();
        let loaded = CookieJar::load_from_file(&path).unwrap();

        // Check that the cookie entry has preserved attributes
        let entries = loaded.cookies.get("example.com").unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.name, "id");
        assert_eq!(entry.value, "42");
        assert_eq!(entry.path.as_deref(), Some("/"));
        assert_eq!(entry.domain.as_deref(), Some(".example.com"));
        assert!(entry.secure);
        assert!(entry.http_only);

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
}
