//! OxiBrowser — headless browser engine with CDP support.
//!
//! Re-exports core types and exposes the search module for library use.

pub mod search;

// Browser types require oxibrowser-core (→ boa_engine JS engine).
// Gated behind the `browser` feature so lightweight consumers (e.g. oxi-agent's
// web search tool) can use the search module without compiling boa_engine.
#[cfg(feature = "browser")]
pub use oxibrowser_core::Browser;
#[cfg(feature = "browser")]
pub use oxibrowser_core::BrowserConfig;
#[cfg(feature = "browser")]
pub use oxibrowser_core::error::Result;

// Convenience re-exports for search types (always available — search only needs reqwest).
pub use search::engine::{GitHubExtra, SearchEngine, SearchError, SearchOutput, SearchResult};
