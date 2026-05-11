//! OxiBrowser Core — Browser lifecycle, Session, Page, and Frame management.
//!
//! Architecture mirrors Lightpanda's Browser → Session → Page → Frame hierarchy,
//! but uses Servo's html5ever for HTML parsing and (optionally) the servo crate
//! for full offscreen rendering.

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
