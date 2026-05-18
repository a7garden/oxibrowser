//! CSS text-based screenshot rendering.
//!
//! Provides:
//! - `render_to_text` / `render_to_markdown`: ASCII/Unicode DOM text rendering
//! - `text_to_png`: PNG screenshot of DOM text content (bitmap font)

mod layout;
mod render;
mod screenshot;
mod visual;

pub use layout::{ComputedStyle, LayoutEngine, LayoutRect};
pub use render::{render_to_markdown, render_to_text};
pub use screenshot::text_to_png;
pub use visual::{render_accessibility_tree, render_box_model_png};
