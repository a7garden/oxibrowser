//! Page — container for a document and its frames.
//!

//! Page — container for a document and its frames.

use crate::error::{CoreError, Result};
use crate::frame::Frame;
use crate::network::resource::Resource;
use std::sync::atomic::{AtomicU32, Ordering};
use tracing::info;
use url::Url;

/// Unique page ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageId(u32);

impl PageId {
    fn next() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl std::fmt::Display for PageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "page-{}", self.0)
    }
}

/// A loaded web page with its DOM tree, resources, and metadata.
pub struct Page {
    /// Unique ID.
    id: PageId,
    /// Page URL.
    url: Url,
    /// Root frame (the main document).
    root_frame: Frame,
    /// HTTP status code.
    status: u16,
    /// Content-Type of the response.
    content_type: String,
    /// Loaded sub-resources.
    resources: Vec<Resource>,
    /// Page title (extracted from <title>).
    title: Option<String>,
}

impl Page {
    /// Create a page from HTML content.
    #[tracing::instrument(skip(html), err)]
    pub async fn from_html(
        url: Url,
        html: &str,
        status: u16,
        content_type: String,
    ) -> Result<Self> {
        let id = PageId::next();
        let root_frame = Frame::from_html(url.clone(), html).await?;

        // Extract title from the frame's DOM
        let title = root_frame.extract_title();

        info!(id = %id, url = %url, status, "page created");

        Ok(Self {
            id,
            url,
            root_frame,
            status,
            content_type,
            resources: Vec::new(),
            title,
        })
    }

    /// Get the page URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Get the page title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Get the page's HTML content.
    pub fn content(&self) -> &str {
        self.root_frame.html()
    }

    /// Get the root frame.
    pub fn root_frame(&self) -> &Frame {
        &self.root_frame
    }

    /// Get the root frame mutably.
    pub fn root_frame_mut(&mut self) -> &mut Frame {
        &mut self.root_frame
    }

    /// Get the HTTP status code.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Get the Content-Type.
    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    /// Get loaded sub-resources.
    pub fn resources(&self) -> &[Resource] {
        &self.resources
    }

    /// Add a loaded resource.
    pub fn add_resource(&mut self, resource: Resource) {
        self.resources.push(resource);
    }

    /// Render the page to a Markdown representation.
    pub fn to_markdown(&self) -> String {
        self.root_frame.to_markdown()
    }

    /// Render the page as text/ASCII art for terminal output.
    pub fn to_text_screenshot(&self) -> String {
        let snapshot = self.root_frame.to_dom_snapshot();
        crate::css::render_to_text(&snapshot)
    }

    /// Render the page as a PNG screenshot.
    ///
    /// Renders the DOM text content as a PNG image using a monospace bitmap font.
    pub fn to_screenshot_png(&self, viewport_width: u32) -> Result<Vec<u8>> {
        let text = self.to_text_screenshot();
        crate::css::text_to_png(&text, viewport_width).map_err(CoreError::ScreenshotError)
    }

    /// Get the page ID.
    pub fn id(&self) -> PageId {
        self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::resource::ResourceType;
    use bytes::Bytes;

    fn make_test_html(title: &str) -> String {
        format!(
            "<!DOCTYPE html><html><head><title>{title}</title></head><body><p>Hello</p></body></html>"
        )
    }

    #[tokio::test]
    async fn test_page_from_html_extracts_title() {
        let url = Url::parse("https://example.com/").unwrap();
        let html = make_test_html("Test Page Title");
        let page = Page::from_html(url, &html, 200, "text/html".to_string())
            .await
            .unwrap();

        assert_eq!(page.title(), Some("Test Page Title"));
    }

    #[tokio::test]
    async fn test_page_content_returns_html() {
        let url = Url::parse("https://example.com/").unwrap();
        let html = make_test_html("Content Test");
        let page = Page::from_html(url, &html, 200, "text/html".to_string())
            .await
            .unwrap();

        let content = page.content();
        assert!(
            content.contains("Hello"),
            "content should contain body text"
        );
        assert!(
            content.contains("<html"),
            "content should contain HTML tags"
        );
    }

    #[tokio::test]
    async fn test_page_to_text_screenshot_non_empty() {
        let url = Url::parse("https://example.com/").unwrap();
        let html = make_test_html("Screenshot Test");
        let page = Page::from_html(url, &html, 200, "text/html".to_string())
            .await
            .unwrap();

        let text = page.to_text_screenshot();
        assert!(!text.is_empty(), "text screenshot should not be empty");
    }

    #[tokio::test]
    async fn test_page_to_screenshot_png_valid_header() {
        let url = Url::parse("https://example.com/").unwrap();
        let html =
            "<!DOCTYPE html><html><head><title>PNG</title></head><body><p>X</p></body></html>";
        let page = Page::from_html(url, html, 200, "text/html".to_string())
            .await
            .unwrap();

        let png = page
            .to_screenshot_png(800)
            .expect("PNG generation should succeed");
        // PNG magic header: 137 80 78 71 13 10 26 10
        assert!(png.len() > 8, "PNG data should be more than 8 bytes");
        assert_eq!(
            &png[0..4],
            &[0x89, 0x50, 0x4E, 0x47],
            "should start with PNG magic"
        );
    }

    #[tokio::test]
    async fn test_page_add_resource_tracks_resources() {
        let url = Url::parse("https://example.com/").unwrap();
        let html = make_test_html("Resource Test");
        let mut page = Page::from_html(url, &html, 200, "text/html".to_string())
            .await
            .unwrap();

        assert!(page.resources().is_empty(), "initially no resources");

        let resource = Resource {
            url: "https://example.com/style.css".to_string(),
            resource_type: ResourceType::Stylesheet,
            status: 200,
            mime_type: "text/css".to_string(),
            body: Bytes::from_static(b"body { color: red; }"),
            loaded_at: std::time::Instant::now(),
        };

        page.add_resource(resource);
        assert_eq!(page.resources().len(), 1);
        assert_eq!(page.resources()[0].url, "https://example.com/style.css");
        assert_eq!(page.resources()[0].resource_type, ResourceType::Stylesheet);
    }
}
