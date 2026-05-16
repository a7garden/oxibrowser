//! BrowseResult — unified page content result for agent consumption.
//!
//! Every navigation method (goto, back, forward, reload, post) returns a
//! `BrowseResult`. This eliminates the need to chain `page() → title() →
//! to_markdown()` for the common "what's on this page?" query.

use crate::page::Page;
use serde::{Deserialize, Serialize};

/// A single object answering "what's on this page?"
///
/// Combines URL, title, HTTP status, markdown, and raw HTML into one
/// serializable result that agents can use directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseResult {
    /// Final URL after redirects.
    pub url: String,
    /// Page `<title>` text.
    pub title: String,
    /// HTTP status code.
    pub status: u16,
    /// Page content rendered as Markdown (agent's primary content).
    pub markdown: String,
    /// Raw HTML source.
    pub html: String,
}

impl BrowseResult {
    /// An empty result (no page loaded).
    pub fn empty() -> Self {
        Self {
            url: String::new(),
            title: String::new(),
            status: 0,
            markdown: String::new(),
            html: String::new(),
        }
    }

    /// Build a `BrowseResult` from a loaded `Page`.
    pub(crate) fn from_page(page: &Page) -> Self {
        Self {
            url: page.url().to_string(),
            title: page.title().unwrap_or("").to_string(),
            status: page.status(),
            markdown: page.to_markdown(),
            html: page.content().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browse_result_empty() {
        let r = BrowseResult::empty();
        assert!(r.url.is_empty());
        assert!(r.title.is_empty());
        assert_eq!(r.status, 0);
        assert!(r.markdown.is_empty());
        assert!(r.html.is_empty());
    }

    #[test]
    fn test_browse_result_serde_roundtrip() {
        let r = BrowseResult {
            url: "https://example.com/".into(),
            title: "Example".into(),
            status: 200,
            markdown: "# Example\nHello".into(),
            html: "<h1>Example</h1>".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let r2: BrowseResult = serde_json::from_str(&json).unwrap();
        assert_eq!(r2.url, r.url);
        assert_eq!(r2.title, r.title);
        assert_eq!(r2.status, r.status);
        assert_eq!(r2.markdown, r.markdown);
        assert_eq!(r2.html, r.html);
    }
}
