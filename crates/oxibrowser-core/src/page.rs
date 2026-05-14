//! Page — container for a document and its frames.
//!
//! Mirrors Lightpanda's `Page.zig`: owns the DOM factory, JS identity map,
//! and frame tree. A Page is created on navigation and holds the root Frame.

use crate::error::Result;
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
    pub fn to_screenshot_png(&self, viewport_width: u32) -> Vec<u8> {
        let text = self.to_text_screenshot();
        crate::css::text_to_png(&text, viewport_width)
    }

    /// Get the page ID.
    pub fn id(&self) -> PageId {
        self.id
    }
}
