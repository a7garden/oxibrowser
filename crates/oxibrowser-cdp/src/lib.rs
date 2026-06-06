//! OxiBrowser CDP — Chrome DevTools Protocol server.
//!
//! Implements the CDP WebSocket protocol so that tools like Puppeteer and
//! Playwright can connect to OxiBrowser, just like they connect to Chrome.
//!

pub mod event;
pub mod protocol;
pub mod server;
pub mod session;

pub mod domains;

pub use server::CdpServer;
