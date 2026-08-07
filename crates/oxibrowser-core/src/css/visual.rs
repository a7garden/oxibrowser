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
pub fn render_box_model_png(
    snapshot: &DomSnapshot,
    viewport_width: u32,
) -> Result<Vec<u8>, String> {
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
        1280,
        720 // viewport
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
fn render_subtree(snapshot: &DomSnapshot, node_id: u32, img: &mut RgbaImage, viewport_w: u32) {
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
fn draw_scaled_text(
    img: &mut RgbaImage,
    text: &str,
    px: u32,
    py: u32,
    color: Rgba<u8>,
    scale: u32,
) {
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
    let c = color.trim().to_lowercase();

    if c == "transparent" || c == "none" {
        return Rgba([0, 0, 0, 0]);
    }
    if c == "currentcolor" {
        return if is_background {
            Rgba([255, 255, 255, 0])
        } else {
            Rgba([0, 0, 0, 255])
        };
    }

    // Named colors (subset of CSS named colors)
    match c.as_str() {
        "white" => return Rgba([255, 255, 255, 255]),
        "black" => return Rgba([0, 0, 0, 255]),
        "red" => return Rgba([255, 0, 0, 255]),
        "green" => return Rgba([0, 128, 0, 255]),
        "blue" => return Rgba([0, 0, 255, 255]),
        "yellow" => return Rgba([255, 255, 0, 255]),
        "cyan" => return Rgba([0, 255, 255, 255]),
        "magenta" => return Rgba([255, 0, 255, 255]),
        "gray" | "grey" => return Rgba([128, 128, 128, 255]),
        "silver" => return Rgba([192, 192, 192, 255]),
        "orange" => return Rgba([255, 165, 0, 255]),
        "purple" => return Rgba([128, 0, 128, 255]),
        "pink" => return Rgba([255, 192, 203, 255]),
        "brown" => return Rgba([165, 42, 42, 255]),
        "navy" => return Rgba([0, 0, 128, 255]),
        "teal" => return Rgba([0, 128, 128, 255]),
        "olive" => return Rgba([128, 128, 0, 255]),
        "maroon" => return Rgba([128, 0, 0, 255]),
        "lime" => return Rgba([0, 255, 0, 255]),
        "aqua" => return Rgba([0, 255, 255, 255]),
        "fuchsia" => return Rgba([255, 0, 255, 255]),
        "limegreen" => return Rgba([50, 205, 50, 255]),
        "hotpink" => return Rgba([255, 105, 180, 255]),
        "gold" => return Rgba([255, 215, 0, 255]),
        "indianred" => return Rgba([205, 92, 92, 255]),
        "khaki" => return Rgba([240, 230, 140, 255]),
        "coral" => return Rgba([255, 127, 80, 255]),
        "salmon" => return Rgba([250, 128, 114, 255]),
        "wheat" => return Rgba([245, 222, 179, 255]),
        "violet" => return Rgba([238, 130, 238, 255]),
        "plum" => return Rgba([221, 160, 221, 255]),
        "orchid" => return Rgba([218, 112, 214, 255]),
        "tomato" => return Rgba([255, 99, 71, 255]),
        "crimson" => return Rgba([220, 20, 60, 255]),
        "forestgreen" => return Rgba([34, 139, 34, 255]),
        "seagreen" => return Rgba([46, 139, 87, 255]),
        "darkgreen" => return Rgba([0, 100, 0, 255]),
        "skyblue" => return Rgba([135, 206, 235, 255]),
        "steelblue" => return Rgba([70, 130, 180, 255]),
        "royalblue" => return Rgba([65, 105, 225, 255]),
        "midnightblue" => return Rgba([25, 25, 112, 255]),
        "slateblue" => return Rgba([106, 90, 205, 255]),
        "dodgerblue" => return Rgba([30, 144, 255, 255]),
        "deepskyblue" => return Rgba([0, 191, 255, 255]),
        "turquoise" => return Rgba([64, 224, 208, 255]),
        "darkorange" => return Rgba([255, 140, 0, 255]),
        "chocolate" => return Rgba([210, 105, 30, 255]),
        "sandybrown" => return Rgba([244, 164, 96, 255]),
        "tan" => return Rgba([210, 180, 140, 255]),
        "peru" => return Rgba([205, 133, 63, 255]),
        "sienna" => return Rgba([160, 82, 45, 255]),
        "rosybrown" => return Rgba([188, 143, 143, 255]),
        "thistle" => return Rgba([216, 191, 216, 255]),
        "lavender" => return Rgba([230, 230, 250, 255]),
        "mistyrose" => return Rgba([255, 228, 225, 255]),
        "snow" => return Rgba([255, 250, 250, 255]),
        "honeydew" => return Rgba([240, 255, 240, 255]),
        "azure" => return Rgba([240, 255, 255, 255]),
        "ivory" => return Rgba([255, 255, 240, 255]),
        "beige" => return Rgba([245, 245, 220, 255]),
        "linen" => return Rgba([250, 240, 230, 255]),
        "ghostwhite" => return Rgba([248, 248, 255, 255]),
        "floralwhite" => return Rgba([255, 250, 240, 255]),
        "aliceblue" => return Rgba([240, 248, 255, 255]),
        "oldlace" => return Rgba([253, 245, 230, 255]),
        "cornsilk" => return Rgba([255, 248, 220, 255]),
        "papayawhip" => return Rgba([255, 239, 213, 255]),
        "antiquewhite" => return Rgba([250, 235, 215, 255]),
        "blanchedalmond" => return Rgba([255, 235, 205, 255]),
        "bisque" => return Rgba([255, 228, 196, 255]),
        "peachpuff" => return Rgba([255, 218, 185, 255]),
        "navajowhite" => return Rgba([255, 222, 173, 255]),
        "moccasin" => return Rgba([255, 228, 181, 255]),
        "gainsboro" => return Rgba([220, 220, 220, 255]),
        "lightgray" | "lightgrey" => return Rgba([211, 211, 211, 255]),
        "darkgray" | "darkgrey" => return Rgba([169, 169, 169, 255]),
        "dimgray" | "dimgrey" => return Rgba([105, 105, 105, 255]),
        "lightsteelblue" => return Rgba([176, 196, 222, 255]),
        "lightblue" => return Rgba([173, 216, 230, 255]),
        "powderblue" => return Rgba([176, 224, 230, 255]),
        "cadetblue" => return Rgba([95, 158, 160, 255]),
        "darkturquoise" => return Rgba([0, 206, 209, 255]),
        "mediumturquoise" => return Rgba([72, 209, 204, 255]),
        "darkcyan" => return Rgba([0, 139, 139, 255]),
        "lightcyan" => return Rgba([224, 255, 255, 255]),
        "paleturquoise" => return Rgba([175, 238, 238, 255]),
        "aquamarine" => return Rgba([127, 255, 212, 255]),
        "mediumaquamarine" => return Rgba([102, 205, 170, 255]),
        "darkseagreen" => return Rgba([143, 188, 143, 255]),
        "mediumseagreen" => return Rgba([60, 179, 113, 255]),
        "lightgreen" => return Rgba([144, 238, 144, 255]),
        "palegreen" => return Rgba([152, 251, 152, 255]),
        "springgreen" => return Rgba([0, 255, 127, 255]),
        "lawngreen" => return Rgba([124, 252, 0, 255]),
        "chartreuse" => return Rgba([127, 255, 0, 255]),
        "greenyellow" => return Rgba([173, 255, 47, 255]),
        "darkolivegreen" => return Rgba([85, 107, 47, 255]),
        "yellowgreen" => return Rgba([154, 205, 50, 255]),
        "olivedrab" => return Rgba([107, 142, 35, 255]),
        "darkkhaki" => return Rgba([189, 183, 107, 255]),
        "palegoldenrod" => return Rgba([238, 232, 170, 255]),
        "lightgoldenrod" => return Rgba([250, 250, 210, 255]),
        "darkgoldenrod" => return Rgba([184, 134, 11, 255]),
        "goldenrod" => return Rgba([218, 165, 32, 255]),
        "darkred" => return Rgba([139, 0, 0, 255]),
        _ => {}
    }

    // #RGB → #RRGGBB
    if c.starts_with('#') && c.len() == 4 {
        let r = &c[1..2];
        let g = &c[2..3];
        let b = &c[3..4];
        let hex = format!("{}{}{}{}{}{}", r, r, g, g, b, b);
        if let (Ok(rv), Ok(gv), Ok(bv)) = (
            u8::from_str_radix(&hex[0..2], 16),
            u8::from_str_radix(&hex[2..4], 16),
            u8::from_str_radix(&hex[4..6], 16),
        ) {
            return Rgba([rv, gv, bv, 255]);
        }
    }

    // #RRGGBB
    if c.starts_with('#')
        && c.len() == 7
        && let (Ok(rv), Ok(gv), Ok(bv)) = (
            u8::from_str_radix(&c[1..3], 16),
            u8::from_str_radix(&c[3..5], 16),
            u8::from_str_radix(&c[5..7], 16),
        )
    {
        return Rgba([rv, gv, bv, 255]);
    }

    // rgb(r, g, b) or rgba(r, g, b, a) — supports 0-255, 0-100%, or 0.0-1.0
    if c.starts_with("rgba(") || c.starts_with("rgb(") {
        let inner = &c[4..c.len() - 1];
        if let Some((r_str, rest)) = split_first_comma(inner)
            && let Some((g_str, rest2)) = split_first_comma(rest)
        {
            let (b_str, a_str) = split_first_comma(rest2).unwrap_or((rest2, "1"));
            if let (Some(rv), Some(gv), Some(bv)) = (
                parse_color_component(r_str),
                parse_color_component(g_str),
                parse_color_component(b_str),
            ) {
                let a = if c.starts_with("rgba(") {
                    parse_alpha(a_str).unwrap_or(1.0)
                } else {
                    1.0
                };
                return Rgba([
                    (rv * 255.0) as u8,
                    (gv * 255.0) as u8,
                    (bv * 255.0) as u8,
                    (a * 255.0) as u8,
                ]);
            }
        }
    }

    // hsl(h, s%, l%) or hsla(h, s%, l%, a)
    if c.starts_with("hsla(") || c.starts_with("hsl(") {
        let inner = &c[4..c.len() - 1];
        if let Some((h_str, rest)) = split_first_comma(inner)
            && let Some((s_str, rest2)) = split_first_comma(rest)
        {
            let (l_str, a_str) = split_first_comma(rest2).unwrap_or((rest2, "1"));
            if let (Some(h), Some(s), Some(l)) = (
                h_str.trim().parse::<f64>().ok(),
                parse_percent(s_str),
                parse_percent(l_str),
            ) {
                let a = if c.starts_with("hsla(") {
                    parse_alpha(a_str).unwrap_or(1.0)
                } else {
                    1.0
                };
                let (r, g, b) = hsl_to_rgb(h, s, l);
                return Rgba([r, g, b, (a * 255.0) as u8]);
            }
        }
    }

    if is_background {
        Rgba([255, 255, 255, 0])
    } else {
        Rgba([0, 0, 0, 255])
    }
}

fn split_first_comma(s: &str) -> Option<(&str, &str)> {
    for (i, ch) in s.char_indices() {
        if ch == ',' {
            return Some((s[..i].trim(), &s[i + 1..]));
        }
    }
    None
}

fn parse_color_component(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(inner) = s.strip_suffix('%') {
        inner.parse::<f64>().ok().map(|v| v / 100.0)
    } else {
        s.parse::<f64>()
            .ok()
            .map(|v| if v > 1.0 { v / 255.0 } else { v })
    }
}

fn parse_percent(s: &str) -> Option<f64> {
    let s = s.trim();
    s.strip_suffix('%')
        .and_then(|inner| inner.parse::<f64>().ok().map(|v| v / 100.0))
}

fn parse_alpha(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(inner) = s.strip_suffix('%') {
        inner.parse::<f64>().ok().map(|v| v / 100.0)
    } else {
        s.parse::<f64>().ok()
    }
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let h = h / 360.0;
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);

    if s == 0.0 {
        let v = (l * 255.0) as u8;
        return (v, v, v);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;

    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);

    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

fn hue_to_rgb(p: f64, q: f64, t: f64) -> f64 {
    let t = if t < 0.0 {
        t + 1.0
    } else if t > 1.0 {
        t - 1.0
    } else {
        t
    };
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
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
    if let Some(ref s) = style
        && s.background_color != "transparent"
        && s.background_color != "#ffffff"
    {
        annotations.push(format!("bg:{}", s.background_color));
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

    // Collect the text already shown in label so we don't duplicate it in children
    let label_trimmed = label.trim().to_lowercase();

    // Recurse into children — skip text nodes that match the parent's label
    for &child_id in &node.children {
        let child_is_duplicate_text = snapshot
            .nodes
            .get(&child_id)
            .map(|cn| cn.node_type == 3 && cn.text_content.trim().to_lowercase() == label_trimmed)
            .unwrap_or(false);
        if child_is_duplicate_text {
            continue; // skip duplicate text
        }
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
            let role = if disabled {
                "button (disabled)"
            } else {
                "button"
            };
            (role.into(), text)
        }
        "INPUT" => {
            let input_type = node
                .attributes
                .get("type")
                .map(|s| s.as_str())
                .unwrap_or("text");
            let placeholder = node
                .attributes
                .get("placeholder")
                .cloned()
                .unwrap_or_default();
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
        "TEXTAREA" => (
            "textbox (multiline)".into(),
            node.attributes
                .get("placeholder")
                .cloned()
                .unwrap_or_default(),
        ),
        "SELECT" => (
            "listbox".into(),
            node.attributes.get("name").cloned().unwrap_or_default(),
        ),
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
        "FORM" => (
            "form".into(),
            node.attributes.get("action").cloned().unwrap_or_default(),
        ),
        "LABEL" => ("label".into(), text),
        "NAV" => ("navigation".into(), String::new()),
        "MAIN" => ("main".into(), String::new()),
        "HEADER" => ("banner".into(), String::new()),
        "FOOTER" => ("contentinfo".into(), String::new()),
        "ASIDE" => ("complementary".into(), String::new()),
        "SECTION" => (
            "region".into(),
            node.attributes
                .get("aria-label")
                .cloned()
                .unwrap_or_default(),
        ),
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

        nodes.insert(
            root_id,
            DomNode {
                id: root_id,
                tag: "html".into(),
                attributes: Default::default(),
                text_content: String::new(),
                children: vec![body_id],
                parent: None,
                node_type: 1,
            },
        );
        nodes.insert(
            body_id,
            DomNode {
                id: body_id,
                tag: "body".into(),
                attributes: Default::default(),
                text_content: String::new(),
                children: vec![div_id],
                parent: Some(root_id),
                node_type: 1,
            },
        );
        nodes.insert(
            div_id,
            DomNode {
                id: div_id,
                tag: "div".into(),
                attributes: {
                    let mut a = std::collections::HashMap::new();
                    a.insert(
                        "style".into(),
                        "width:200px;height:100px;background-color:#336699".into(),
                    );
                    a
                },
                text_content: "Hello".into(),
                children: vec![],
                parent: Some(body_id),
                node_type: 1,
            },
        );

        let mut snap = DomSnapshot::empty();
        snap.url = "http://test/".into();
        snap.nodes = nodes;
        snap.root_id = root_id;
        snap.body_id = Some(body_id);

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

        nodes.insert(
            root_id,
            DomNode {
                id: root_id,
                tag: "html".into(),
                attributes: Default::default(),
                text_content: String::new(),
                children: vec![body_id],
                parent: None,
                node_type: 1,
            },
        );
        nodes.insert(
            body_id,
            DomNode {
                id: body_id,
                tag: "body".into(),
                attributes: Default::default(),
                text_content: String::new(),
                children: vec![h1_id, p_id, btn_id],
                parent: Some(root_id),
                node_type: 1,
            },
        );
        nodes.insert(
            h1_id,
            DomNode {
                id: h1_id,
                tag: "h1".into(),
                attributes: Default::default(),
                text_content: "Title".into(),
                children: vec![],
                parent: Some(body_id),
                node_type: 1,
            },
        );
        nodes.insert(
            p_id,
            DomNode {
                id: p_id,
                tag: "p".into(),
                attributes: Default::default(),
                text_content: "Hello world".into(),
                children: vec![],
                parent: Some(body_id),
                node_type: 1,
            },
        );
        nodes.insert(
            btn_id,
            DomNode {
                id: btn_id,
                tag: "button".into(),
                attributes: Default::default(),
                text_content: "Click".into(),
                children: vec![],
                parent: Some(body_id),
                node_type: 1,
            },
        );

        let mut snap = DomSnapshot::empty();
        snap.url = "http://test/".into();
        snap.nodes = nodes;
        snap.root_id = root_id;
        snap.body_id = Some(body_id);

        let tree = render_accessibility_tree(&snap);
        assert!(
            tree.contains("heading"),
            "Should contain heading role, got: {}",
            tree
        );
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
            <!-- <img> removed: Blitz panics on relative URL resolve for the synthetic
                 base URL used in this test. The accessibility tree is still
                 exercised by the other tags below. -->
            <p style="display:none">Hidden</p>
        </body></html>"##;

        let frame = Frame::from_html(url::Url::parse("http://test/").unwrap(), html)
            .await
            .unwrap();

        let snapshot = crate::js::dom_snapshot::DomSnapshot::from_frame(&frame);
        let tree = render_accessibility_tree(&snapshot);

        assert!(tree.contains("heading"), "Should have heading");
        assert!(tree.contains("Welcome"), "Should have heading text");
        assert!(tree.contains("paragraph"), "Should have paragraph");
        assert!(
            tree.contains("interactive"),
            "Should have interactive elements"
        );
    }

    #[tokio::test]
    async fn test_box_model_screenshot_realistic() {
        use crate::frame::Frame;

        let html = r##"<html><body>
            <h1>Title</h1>
            <p>Text</p>
            <div style="width:200px;height:50px;background-color:#336699">Box</div>
        </body></html>"##;

        let frame = Frame::from_html(url::Url::parse("http://test/").unwrap(), html)
            .await
            .unwrap();

        let snapshot = crate::js::dom_snapshot::DomSnapshot::from_frame(&frame);
        let png = render_box_model_png(&snapshot, 640).unwrap();

        assert!(png.len() > 100, "PNG should have meaningful size");
        assert_eq!(&png[0..4], b"\x89PNG", "Should be valid PNG");
    }
}
