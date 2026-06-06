//! OxiBrowser — headless browser engine with CDP support.
//!
//! Re-exports core types and exposes the search module for library use.

pub mod search;

pub use oxibrowser_core::error::Result;
pub use oxibrowser_core::Browser;
pub use oxibrowser_core::BrowserConfig;

// Convenience re-exports for search types.
pub use search::engine::{GitHubExtra, SearchEngine, SearchError, SearchOutput, SearchResult};
