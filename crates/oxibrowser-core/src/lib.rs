//! OxiBrowser Core — Browser lifecycle, Session, Page, and Frame management.
//!
//! Architecture mirrors Lightpanda's Browser → Session → Page → Frame hierarchy,
//! using Servo's html5ever for HTML parsing and boa_engine for JavaScript execution.

pub mod browser;
pub mod config;
pub mod frame;
pub mod page;
pub mod session;

pub mod network;
pub mod js;

pub mod error;

pub use browser::Browser;
pub use config::BrowserConfig;
pub use error::Result;
