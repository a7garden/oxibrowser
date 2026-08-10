//! OxiBrowser Core — Browser lifecycle, Session, Page, and Frame management.
//!
//! Uses Servo's html5ever for HTML parsing and boa_engine for JavaScript execution.

pub mod browse_result;
pub mod browser;
pub mod challenge;
pub mod config;
pub mod css;
pub mod event;
pub mod extract;
pub mod frame;
pub mod fonts;
pub mod page;
pub mod script;
pub mod session;
pub mod tab;

pub mod js;
pub mod network;

pub mod encoding;

pub mod error;

/// Blank white PNG fallback (re-exported from the render crate).
pub use oxibrowser_render::blank_png;

pub use browse_result::BrowseResult;
pub use browser::Browser;
pub use config::BrowserConfig;
pub use config::BrowserConfigBuilder;
pub use error::Result;
pub use event::BrowserEvent;
pub use tab::Tab;
