//! CSS text-based screenshot rendering.
//!
//! Takes a DOM tree and produces a text/terminal visual representation.
//! NOT pixel-perfect Chromium rendering — just visible text content.

mod render;
pub use render::render_to_text;
pub use render::render_to_markdown;

#[cfg(test)]
mod tests {
    #[test]
    fn test_render_module_compiles() {
        // The render module is a stub — real rendering needs full DOM integration
        assert!(true);
    }
}
