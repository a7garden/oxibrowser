//! CSS text-based + accessibility rendering helpers.
//!
//! Provides:
//! - `render_to_text` / `render_to_markdown`: ASCII/Unicode DOM text rendering
//! - `render_accessibility_tree` / `render_box_model_png`: box-model visuals
//!
//! The legacy bitmap-font screenshot renderer (`text_to_png`) has been retired
//! in favour of the Blitz-backed `oxibrowser_render` pipeline; screenshot
//! capture now lives in `oxibrowser_render` (with a `blank_png` fallback).

mod layout;
mod render;
mod visual;

pub use layout::{ComputedStyle, LayoutEngine, LayoutRect};
pub use render::{render_to_markdown, render_to_text};
pub use visual::{render_accessibility_tree, render_box_model_png};
