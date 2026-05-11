//! JavaScript runtime abstraction.
//!
//! When the `full-servo` feature is enabled, uses Servo's JS engine
//! (SpiderMonkey) via the servo crate's `WebView::evaluate_javascript()`.
//! Otherwise, provides a minimal JS evaluation stub.

pub mod runtime;

pub use runtime::JsRuntime;
