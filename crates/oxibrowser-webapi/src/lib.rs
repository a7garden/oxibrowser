//! OxiBrowser WebAPI — DOM and Web API implementations.
//!
//! Provides DOM parsing via html5ever (from the Servo ecosystem) and
//! basic WebAPI types needed for browser automation.

pub mod dom;

pub use dom::Document;
