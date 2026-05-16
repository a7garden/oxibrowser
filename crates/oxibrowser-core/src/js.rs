//! JavaScript runtime abstraction.
//!
//! Uses **boa_engine** (pure Rust JavaScript engine) for real JS execution.
//! No C dependencies — no V8, no SpiderMonkey, no Node.js.

pub mod dom_snapshot;
pub mod form;
pub mod input;
pub mod job_queue;
pub mod mouse;
pub mod runtime;

// ─── Re-exports ───────────────────────────────────────────────────────────────

// DOM bridge
pub use dom_snapshot::{DomMutation, DomSnapshot};

// Input (keyboard + mouse + insertText)
pub use input::{js_dispatch_key_event, js_dispatch_mouse_event, js_insert_text};

// Mouse (hover, drag, scroll, double-click, right-click, move)
pub use mouse::{
    js_double_click, js_drag, js_hover, js_move_mouse, js_right_click, js_scroll,
    js_scroll_into_view, key_to_code, parse_key_combo,
};

// Form (fill, select, check, upload)
pub use form::{js_check, js_clear, js_fill, js_get_value, js_select_option, js_upload_file};

// Job queue (timer bridge)
pub use job_queue::TokioJobQueue;

// Runtime (boa context)
pub use runtime::{JsEvalResult, JsRuntime, JsRuntimeConfig};
