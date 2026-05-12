//! JavaScript runtime abstraction.
//!
//! Uses **boa_engine** (pure Rust JavaScript engine) for real JS execution.
//! No C dependencies — no V8, no SpiderMonkey, no Node.js.

pub mod runtime;

pub use runtime::JsRuntime;
