//! Box-model PNG screenshot renderer using LayoutEngine.
//!
//! Renders DOM elements as colored rectangles with text labels,
//! using computed styles and estimated positions from LayoutEngine.
//! This gives AI agents a visual approximation of page layout without
//! a real CSS rendering engine.

use crate::css::{ComputedStyle, LayoutEngine};
use crate::js::dom_snapshot::{DomNode, DomSnapshot};
use image::{ImageBuffer, Rgba, RgbaImage};
use std::io::Cursor;

const CHAR_W: u32 = 8;
const CHAR_H: u32 = 16;
const MAX_IMAGE_HEIGHT: u32 = 16384;
const FONT_DATA: &[u8] = include_bytes!("font_8x16.bin");
const GLYPH_COUNT: usize = 95;

/// Render a DomSnapshot as a box-model PNG image.
///
/// Each visible element is drawn as a colored rectangle at its estimated
/// position. Text content is rendered inside. Hidden elements are skipped.
pub fn render_box_model_png(snapshot: &DomSnapshot, viewport_width: u32) -> Result<Vec<u8>, String> {
    let vw = viewport_width.max(320);
    let img_h = estimate_page_height(snapshot, vw);
    let img_h = img_h.min(MAX_IMAGE_HEIGHT);

    // White background
    let mut img: RgbaImage = ImageBuffer::from_pixel(vw, img_h, Rgba([255, 255, 255, 255]));

    // Render from body
    if let Some(body_id) = snapshot.body_id {
        render_subtree(snapshot, body_id, &mut img, vw);
    }

    encode_png(&img)
}

/// Render the accessibility tree as a structured string.
///
/// Returns a text representation of the page's semantic structure,
/// showing what a user (or screen reader) would perceive.
pub fn render_accessibility_tree(snapshot: &DomSnapshot) -> String {
    let mut output = String::new();

    let body_id = match snapshot.body_id {
        Some(id) => id,
        None => return "(empty page)".into(),
    };

    output.push_str(&format!(
        "page ({}×{})\n",
        1280, 720 // viewport
    ));

    if let Some(body) = snapshot.nodes.get(&body_id) {
        for &child_id in &body.children {
            build_a11y_node(snapshot, child_id, &mut output, 1);
        }
    }

    output.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

/// Estimate total page height from the snapshot.
fn estimate_page_height(snapshot: &DomSnapshot, _viewport_width: u32) -> u32 {
    let body_id = match snapshot.body_id {
        Some(id) => id,
        None => return 720,
    };

    let mut max_bottom = 720.0f64;
    if let Some(body) = snapshot.nodes.get(&body_id) {
        for &child_id in &body.children {
            let rect = LayoutEngine::compute_rect(snapshot, child_id);
            let bottom = rect.top + rect.height;
            if bottom > max_bottom {
                max_bottom = bottom;
            }
        }
    }

    (max_bottom + 40.0) as u32 // 40px bottom padding
}

/// Recursively render a subtree of nodes as colored boxes.
fn render_subtree(
    snapshot: &DomSnapshot,
    node_id: u32,
    img: &mut RgbaImage,
    viewport_w: u32,
) {
    let node = match snapshot.nodes.get(&node_id) {
        Some(n) => n,
        None => return,
    };

    if node.node_type != 1 {
        return; // Skip text nodes, comments, etc.
    }

    let style = LayoutEngine::compute_style(snapshot, node_id);

    let style = match style {
        Some(s) if s.visible => s,
        _ => return, // Skip invisible
    };

    let rect = LayoutEngine::compute_rect(snapshot, node_id);
    let tag = node.tag.to_uppercase();

    // Skip invisible tags
    if matches!(
        tag.as_str(),
        "SCRIPT" | "STYLE" | "META" | "LINK" | "HEAD" | "NOSCRIPT" | "BASE"
    ) {
        return;
    }

    // Draw the box
    let x = rect.left as u32;
    let y = (rect.top) as u32;
    let w = rect.width as u32;
    let h = rect.height as u32;

    if w == 0 || h == 0 || x >= viewport_w || y >= img.height() {
        return;
    }

    // Background color
    let bg_color = parse_color_to_rgba(&style.background_color, true);
    let border_color = parse_color_to_rgba(&style.color, false);
    let text_color = parse_color_to_rgba(&style.color, false);

    // Draw background fill
    let effective_w = (x + w).min(viewport_w) - x;
    let effective_h = (y + h).min(img.height()) - y;

    if effective_w > 0 && effective_h > 0 {
        draw_filled_rect(img, x, y, effective_w, effective_h, bg_color);
        draw_rect_outline(img, x, y, effective_w, effective_h, border_color);
    }

    // Draw text content
    let text = node.text_content.trim();
    if !text.is_empty() {
        let font_size = style.font_size;
        let scale = ((font_size / 16.0).clamp(0.5, 3.0) as u32).max(1);
        let text_x = x + 4;
        let text_y = y + 4;

        // Only first line for space reasons
        let first_line = text.lines().next().unwrap_or("");
        let max_chars = (effective_w.saturating_sub(8) / (CHAR_W * scale)) as usize;
        let truncated: String = first_line.chars().take(max_chars).collect();

        if !truncated.is_empty() {
            draw_scaled_text(img, &truncated, text_x, text_y, text_color, scale);
        }
    }

    // Render children
    for &child_id in &node.children {
        render_subtree(snapshot, child_id, img, viewport_w);
    }
}

/// Draw a filled rectangle.
fn draw_filled_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: Rgba<u8>) {
    let img_w = img.width();
    let img_h = img.height();
    for py in y..(y + h).min(img_h) {
        for px in x..(x + w).min(img_w) {
            let existing = img.get_pixel(px, py);
            // Alpha blend
            let alpha = color.0[3] as f64 / 255.0;
            let r = (color.0[0] as f64 * alpha + existing.0[0] as f64 * (1.0 - alpha)) as u8;
            let g = (color.0[1] as f64 * alpha + existing.0[1] as f64 * (1.0 - alpha)) as u8;
            let b = (color.0[2] as f64 * alpha + existing.0[2] as f64 * (1.0 - alpha)) as u8;
            img.put_pixel(px, py, Rgba([r, g, b, 255]));
        }
    }
}

/// Draw a rectangle outline (1px border).
fn draw_rect_outline(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: Rgba<u8>) {
    let img_w = img.width();
    let img_h = img.height();

    // Top and bottom
    for px in x..(x + w).min(img_w) {
        if y < img_h {
            img.put_pixel(px, y, color);
        }
        if y + h > 0 && y + h - 1 < img_h {
            img.put_pixel(px, y + h - 1, color);
        }
    }
    // Left and right
    for py in y..(y + h).min(img_h) {
        if x < img_w {
            img.put_pixel(x, py, color);
        }
        if x + w > 0 && x + w - 1 < img_w {
            img.put_pixel(x + w - 1, py, color);
        }
    }
}

/// Draw text with optional scaling.
fn draw_scaled_text(img: &mut RgbaImage, text: &str, px: u32, py: u32, color: Rgba<u8>, scale: u32) {
    let scale = scale.max(1);
    let mut cx = px;
    for ch in text.chars() {
        let code = ch as u32;
        if !(32..=126).contains(&code) {
            continue;
        }
        let glyph_idx = (code - 32) as usize;
        if glyph_idx >= GLYPH_COUNT {
            continue;
        }
        let offset = glyph_idx * CHAR_H as usize;
        for row in 0..CHAR_H {
            let byte = FONT_DATA[offset + row as usize];
            for col in 0..CHAR_W {
                if byte & (0x80 >> col) != 0 {
                    // Draw scaled pixel
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let x = cx + col * scale + sx;
                            let y = py + row * scale + sy;
                            if x < img.width() && y < img.height() {
                                img.put_pixel(x, y, color);
                            }
                        }
                    }
                }
            }
        }
        cx += CHAR_W * scale;
        if cx >= img.width() {
            break;
        }
    }
}

/// Parse a CSS color string to RGBA.
fn parse_color_to_rgba(color: &str, is_background: bool) -> Rgba<u8> {
    if color == "transparent" {
        return Rgba([0, 0, 0, 0]);
    }
    if color.starts_with('#') && color.len() == 7 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&color[1..3], 16),
            u8::from_str_radix(&color[3..5], 16),
            u8::from_str_radix(&color[5..7], 16),
        ) {
            return Rgba([r, g, b, 255]);
        }
    }
    if is_background {
        Rgba([255, 255, 255, 0]) // transparent for unknown backgrounds
    } else {
        Rgba([0, 0, 0, 255]) // black for unknown text
    }
}

/// Encode RGBA image as PNG bytes.
fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut png_bytes = Vec::new();
    let mut cursor = Cursor::new(&mut png_bytes);
    img.write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|e| format!("PNG encoding failed: {}", e))?;
    Ok(png_bytes)
}

/// Build accessibility tree text for a node and its descendants.
fn build_a11y_node(snapshot: &DomSnapshot, node_id: u32, output: &mut String, depth: usize) {
    let node = match snapshot.nodes.get(&node_id) {
        Some(n) => n,
        None => return,
    };

    if node.node_type == 3 {
        // Text node
        let text = node.text_content.trim();
        if !text.is_empty() {
            let indent = "│   ".repeat(depth.saturating_sub(1));
            output.push_str(&format!("{}├── text \"{}\"\n", indent, truncate(text, 80)));
        }
        return;
    }

    if node.node_type != 1 {
        return;
    }

    let tag = node.tag.to_uppercase();

    // Skip hidden tags
    if matches!(
        tag.as_str(),
        "SCRIPT" | "STYLE" | "META" | "LINK" | "HEAD" | "NOSCRIPT" | "BASE"
    ) {
        return;
    }

    let indent = "│   ".repeat(depth.saturating_sub(1));
    let connector = if depth == 0 { "" } else { "├── " };

    let style = LayoutEngine::compute_style(snapshot, node_id);
    let rect = LayoutEngine::compute_rect(snapshot, node_id);

    let (role, label) = compute_a11y_role(snapshot, node_id, &style);

    let visible = style.as_ref().map(|s| s.visible).unwrap_or(true);
    let interactive = style.as_ref().map(|s| s.interactive).unwrap_or(false);

    // Build annotation
    let mut annotations = Vec::new();
    if !visible {
        annotations.push("hidden".into());
    }
    if interactive {
        annotations.push("interactive".into());
    }
    if rect.width > 0.0 && rect.height > 0.0 {
        annotations.push(format!("at y:{}", rect.top as i32));
    }
    if let Some(ref s) = style {
        if s.background_color != "transparent" && s.background_color != "#ffffff" {
            annotations.push(format!("bg:{}", s.background_color));
        }
    }

    let ann_str = if annotations.is_empty() {
        String::new()
    } else {
        format!(" ({})", annotations.join(", "))
    };

    let label_str = if label.is_empty() {
        String::new()
    } else {
        format!(" \"{}\"", truncate(&label, 60))
    };

    output.push_str(&format!(
        "{}{}{}{}{}\n",
        indent, connector, role, label_str, ann_str
    ));

    // Recurse into children
    for &child_id in &node.children {
        build_a11y_node(snapshot, child_id, output, depth + 1);
    }
}

/// Compute the accessibility role and label for a DOM node.
fn compute_a11y_role(
    snapshot: &DomSnapshot,
    node_id: u32,
    _style: &Option<ComputedStyle>,
) -> (String, String) {
    let node = match snapshot.nodes.get(&node_id) {
        Some(n) => n,
        None => return ("unknown".into(), String::new()),
    };

    let tag = node.tag.to_uppercase();
    let text = node.text_content.trim().to_string();

    // Check for ARIA attributes first
    if let Some(role) = node.attributes.get("role") {
        return (role.clone(), get_label(node));
    }

    let disabled = node.attributes.contains_key("disabled");

    match tag.as_str() {
        "H1" | "H2" | "H3" | "H4" | "H5" | "H6" => {
            let level = tag.strip_prefix('H').unwrap_or("1");
            (format!("heading (level {})", level), text)
        }
        "P" => ("paragraph".into(), text),
        "A" => {
            let href = node.attributes.get("href").cloned().unwrap_or_default();
            ("link".into(), if text.is_empty() { href } else { text })
        }
        "BUTTON" => {
            let role = if disabled { "button (disabled)" } else { "button" };
            (role.into(), text)
        }
        "INPUT" => {
            let input_type = node.attributes.get("type").map(|s| s.as_str()).unwrap_or("text");
            let placeholder = node.attributes.get("placeholder").cloned().unwrap_or_default();
            let name = node.attributes.get("name").cloned().unwrap_or_default();
            let label = if !placeholder.is_empty() {
                placeholder
            } else if !name.is_empty() {
                name
            } else {
                text
            };
            (format!("textbox (type={})", input_type), label)
        }
        "TEXTAREA" => ("textbox (multiline)".into(), node.attributes.get("placeholder").cloned().unwrap_or_default()),
        "SELECT" => ("listbox".into(), node.attributes.get("name").cloned().unwrap_or_default()),
        "OPTION" => ("option".into(), text),
        "IMG" => {
            let alt = node.attributes.get("alt").cloned().unwrap_or_default();
            let src = node.attributes.get("src").cloned().unwrap_or_default();
            ("image".into(), if alt.is_empty() { src } else { alt })
        }
        "UL" | "OL" => ("list".into(), String::new()),
        "LI" => ("listitem".into(), text),
        "TABLE" => ("table".into(), String::new()),
        "TR" => ("row".into(), String::new()),
        "TD" | "TH" => ("cell".into(), text),
        "FORM" => ("form".into(), node.attributes.get("action").cloned().unwrap_or_default()),
        "LABEL" => ("label".into(), text),
        "NAV" => ("navigation".into(), String::new()),
        "MAIN" => ("main".into(), String::new()),
        "HEADER" => ("banner".into(), String::new()),
        "FOOTER" => ("contentinfo".into(), String::new()),
        "ASIDE" => ("complementary".into(), String::new()),
        "SECTION" => ("region".into(), node.attributes.get("aria-label").cloned().unwrap_or_default()),
        "ARTICLE" => ("article".into(), String::new()),
        "SPAN" | "STRONG" | "EM" | "B" | "I" | "U" | "SMALL" | "CODE" => ("text".into(), text),
        "DIV" => ("group".into(), String::new()),
        _ => (tag.to_lowercase(), text),
    }
}

/// Get the accessible label from aria-label, aria-labelledby, title, or text content.
fn get_label(node: &DomNode) -> String {
    if let Some(label) = node.attributes.get("aria-label") {
        return label.clone();
    }
    if let Some(title) = node.attributes.get("title") {
        return title.clone();
    }
    node.text_content.trim().to_string()
}

/// Truncate string to max_len characters with ellipsis.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let mut result: String = s.chars().take(max_len - 1).collect();
    result.push('…');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_box_model_produces_valid_png() {
        let mut nodes = std::collections::HashMap::new();
        let root_id = 1u32;
        let body_id = 2u32;
        let div_id = 3u32;

        nodes.insert(root_id, DomNode {
            id: root_id, tag: "html".into(), attributes: Default::default(),
            text_content: String::new(), children: vec![body_id], parent: None, node_type: 1,
        });
        nodes.insert(body_id, DomNode {
            id: body_id, tag: "body".into(), attributes: Default::default(),
            text_content: String::new(), children: vec![div_id], parent: Some(root_id), node_type: 1,
        });
        nodes.insert(div_id, DomNode {
            id: div_id, tag: "div".into(),
            attributes: {
                let mut a = std::collections::HashMap::new();
                a.insert("style".into(), "width:200px;height:100px;background-color:#336699".into());
                a
            },
            text_content: "Hello".into(), children: vec![], parent: Some(body_id), node_type: 1,
        });

        let snap = DomSnapshot {
            url: "http://test/".into(), title: String::new(),
            nodes, root_id, body_id: Some(body_id), head_id: None,
        };

        let png = render_box_model_png(&snap, 640).unwrap();
        assert!(png.len() > 8);
        assert_eq!(&png[0..4], b"\x89PNG");
    }

    #[test]
    fn test_accessibility_tree_basic() {
        let mut nodes = std::collections::HashMap::new();
        let root_id = 1u32;
        let body_id = 2u32;
        let h1_id = 3u32;
        let p_id = 4u32;
        let btn_id = 5u32;

        nodes.insert(root_id, DomNode {
            id: root_id, tag: "html".into(), attributes: Default::default(),
            text_content: String::new(), children: vec![body_id], parent: None, node_type: 1,
        });
        nodes.insert(body_id, DomNode {
            id: body_id, tag: "body".into(), attributes: Default::default(),
            text_content: String::new(), children: vec![h1_id, p_id, btn_id], parent: Some(root_id), node_type: 1,
        });
        nodes.insert(h1_id, DomNode {
            id: h1_id, tag: "h1".into(), attributes: Default::default(),
            text_content: "Title".into(), children: vec![], parent: Some(body_id), node_type: 1,
        });
        nodes.insert(p_id, DomNode {
            id: p_id, tag: "p".into(), attributes: Default::default(),
            text_content: "Hello world".into(), children: vec![], parent: Some(body_id), node_type: 1,
        });
        nodes.insert(btn_id, DomNode {
            id: btn_id, tag: "button".into(), attributes: Default::default(),
            text_content: "Click".into(), children: vec![], parent: Some(body_id), node_type: 1,
        });

        let snap = DomSnapshot {
            url: "http://test/".into(), title: String::new(),
            nodes, root_id, body_id: Some(body_id), head_id: None,
        };

        let tree = render_accessibility_tree(&snap);
        assert!(tree.contains("heading"), "Should contain heading role, got: {}", tree);
        assert!(tree.contains("Title"), "Should contain heading text");
        assert!(tree.contains("paragraph"), "Should contain paragraph role");
        assert!(tree.contains("button"), "Should contain button role");
        assert!(tree.contains("interactive"), "Button should be interactive");
    }

    #[tokio::test]
    async fn test_accessibility_tree_realistic_page() {
        use crate::frame::Frame;
        
        let html = r##"<html><head><title>Demo</title></head><body>
            <h1>Welcome</h1>
            <p style="color:red">Red text here</p>
            <div style="width:300px;height:80px">
                <button disabled>Disabled</button>
                <a href="/about">About</a>
            </div>
            <img src="logo.png" alt="Logo">
            <p style="display:none">Hidden</p>
        </body></html>"##;
        
        let frame = Frame::from_html(
            url::Url::parse("http://test/").unwrap(),
            html,
        ).await.unwrap();
        
        let snapshot = crate::js::dom_snapshot::DomSnapshot::from_frame(&frame);
        let tree = render_accessibility_tree(&snapshot);
        
        assert!(tree.contains("heading"), "Should have heading");
        assert!(tree.contains("Welcome"), "Should have heading text");
        assert!(tree.contains("paragraph"), "Should have paragraph");
        assert!(tree.contains("button"), "Should have button");
        assert!(tree.contains("interactive"), "Should have interactive elements");
        assert!(tree.contains("hidden"), "Should mark hidden elements");
        assert!(tree.contains("image"), "Should have image role");
        assert!(tree.contains("Logo"), "Should have image alt text");
    }

    #[tokio::test]
    async fn test_box_model_screenshot_realistic() {
        use crate::frame::Frame;
        
        let html = r##"<html><body>
            <h1>Title</h1>
            <p>Text</p>
            <div style="width:200px;height:50px;background-color:#336699">Box</div>
        </body></html>"##;
        
        let frame = Frame::from_html(
            url::Url::parse("http://test/").unwrap(),
            html,
        ).await.unwrap();
        
        let snapshot = crate::js::dom_snapshot::DomSnapshot::from_frame(&frame);
        let png = render_box_model_png(&snapshot, 640).unwrap();
        
        assert!(png.len() > 100, "PNG should have meaningful size");
        assert_eq!(&png[0..4], b"\x89PNG", "Should be valid PNG");
    }
}
