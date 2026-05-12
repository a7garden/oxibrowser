//! JavaScript runtime abstraction.
//!
//! Uses **boa_engine** (pure Rust JavaScript engine) for real JS execution.
//! No C dependencies — no V8, no SpiderMonkey, no Node.js.

pub mod dom_snapshot;
pub mod runtime;

pub use dom_snapshot::DomSnapshot;
pub use runtime::{JsEvalResult, JsRuntime, JsRuntimeConfig};
