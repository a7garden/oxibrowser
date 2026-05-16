//! OxiBrowser Core — Browser lifecycle, Session, Page, and Frame management.
//!
//! Architecture mirrors Lightpanda's Browser → Session → Page → Frame hierarchy,
//! using Servo's html5ever for HTML parsing and boa_engine for JavaScript execution.

pub mod browse_result;
pub mod browser;
pub mod config;
pub mod css;
pub mod frame;
pub mod page;
pub mod script;
pub mod session;
pub mod tab;

pub mod js;
pub mod network;

pub mod encoding;

pub mod error;

pub use browse_result::BrowseResult;
pub use browser::Browser;
pub use config::BrowserConfig;
pub use error::Result;
pub use tab::Tab;
