//! JavaScript runtime abstraction.
//!
//! Uses **boa_engine** (pure Rust JavaScript engine) for real JS execution.
//! No C dependencies — no V8, no SpiderMonkey, no Node.js.

pub mod dom_snapshot;
pub mod input;
pub mod job_queue;
pub mod runtime;

pub use dom_snapshot::{DomMutation, DomSnapshot};
pub use input::{js_dispatch_key_event, js_dispatch_mouse_event, js_insert_text};
pub use job_queue::TokioJobQueue;
pub use runtime::{JsEvalResult, JsRuntime, JsRuntimeConfig};
