//! Resource types loaded by pages.

use bytes::Bytes;
use std::time::Instant;

/// MIME type of a loaded resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceType {
    Document,
    Script,
    Stylesheet,
    Image,
    Font,
    Xhr,
    Fetch,
    WebSocket,
    Other(String),
}

impl std::fmt::Display for ResourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceType::Document => write!(f, "Document"),
            ResourceType::Script => write!(f, "Script"),
            ResourceType::Stylesheet => write!(f, "Stylesheet"),
            ResourceType::Image => write!(f, "Image"),
            ResourceType::Font => write!(f, "Font"),
            ResourceType::Xhr => write!(f, "XHR"),
            ResourceType::Fetch => write!(f, "Fetch"),
            ResourceType::WebSocket => write!(f, "WebSocket"),
            ResourceType::Other(s) => write!(f, "Other({s})"),
        }
    }
}

/// A loaded resource with its metadata.
#[derive(Debug, Clone)]
pub struct Resource {
    /// URL of the resource.
    pub url: String,
    /// Resource type.
    pub resource_type: ResourceType,
    /// HTTP status code.
    pub status: u16,
    /// MIME type from Content-Type header.
    pub mime_type: String,
    /// Resource body bytes.
    pub body: Bytes,
    /// When this resource was loaded.
    pub loaded_at: Instant,
}

impl Resource {
    /// Get the body as UTF-8 text.
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.body).ok()
    }

    /// Body size in bytes.
    pub fn size(&self) -> usize {
        self.body.len()
    }
}
