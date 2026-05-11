//! Frame — a document frame within a page.
//!
//! Mirrors Lightpanda's `Frame.zig`: holds the parsed DOM tree, document
//! URL, and child frames. The root Frame represents the main document.

use crate::error::Result;
use oxibrowser_webapi::dom::{Document, NodeId};
use std::sync::atomic::{AtomicU32, Ordering};
use tracing::debug;
use url::Url;

/// Unique frame ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameId(u32);

impl FrameId {
    fn next() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl std::fmt::Display for FrameId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "frame-{}", self.0)
    }
}

/// A document frame with its parsed DOM tree.
pub struct Frame {
    /// Unique ID.
    id: FrameId,
    /// Frame URL.
    url: Url,
    /// Original HTML source.
    html: String,
    /// Parsed DOM document.
    document: Document,
    /// Child frames (iframes).
    children: Vec<Frame>,
    /// DOM version counter (for cache invalidation).
    dom_version: u64,
}

impl Frame {
    /// Parse HTML into a Frame with its DOM tree.
    pub async fn from_html(url: Url, html: &str) -> Result<Self> {
        let id = FrameId::next();
        let document = Document::parse(html);

        debug!(id = %id, url = %url, "frame created");

        Ok(Self {
            id,
            url,
            html: html.to_string(),
            document,
            children: Vec::new(),
            dom_version: 0,
        })
    }

    /// Get the frame ID.
    pub fn id(&self) -> FrameId {
        self.id
    }

    /// Get the frame URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Get the raw HTML.
    pub fn html(&self) -> &str {
        &self.html
    }

    /// Get the parsed document.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Get the parsed document mutably.
    pub fn document_mut(&mut self) -> &mut Document {
        self.dom_version += 1;
        &mut self.document
    }

    /// Get child frames.
    pub fn children(&self) -> &[Frame] {
        &self.children
    }

    /// Add a child frame.
    pub fn add_child(&mut self, frame: Frame) {
        self.dom_version += 1;
        self.children.push(frame);
    }

    /// Get the DOM version (for cache invalidation).
    pub fn dom_version(&self) -> u64 {
        self.dom_version
    }

    /// Extract the page title from the DOM.
    pub fn extract_title(&self) -> Option<String> {
        self.document.query_text("title").or_else(|| {
            // Fallback: extract from HTML with a simple regex-like approach
            let html = &self.html;
            let start = html.find("<title>").map(|i| i + 7)?;
            let end = html.find("</title>")?;
            if start < end {
                Some(html[start..end].trim().to_string())
            } else {
                None
            }
        })
    }

    /// Convert the frame's content to a Markdown string.
    pub fn to_markdown(&self) -> String {
        self.document.to_markdown()
    }

    /// Query the DOM using a CSS selector (basic).
    pub fn query_selector(&self, selector: &str) -> Option<NodeId> {
        self.document.query_selector(selector)
    }

    /// Get the text content of a node.
    pub fn text_content(&self, node_id: NodeId) -> Option<String> {
        self.document.text_content(node_id)
    }

    /// Extract sub-resource URLs from the DOM.
    ///
    /// Finds `<script src>`, `<link href>` (stylesheet), `<img src>`,
    /// `<iframe src>` and returns their URLs.
    pub fn extract_resource_urls(&self) -> Vec<oxibrowser_webapi::dom::ResourceUrl> {
        self.document.extract_resource_urls()
    }
}
