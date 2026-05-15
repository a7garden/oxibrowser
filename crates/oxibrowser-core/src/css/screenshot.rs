//! Text-to-PNG screenshot renderer.
//!
//! Renders a DOM tree snapshot as a PNG image by drawing text using
//! a built-in 8x16 monospace bitmap font. This is NOT pixel-perfect
//! Chromium rendering — it's a text-based visual approximation for
//! terminal-style screenshots.

use image::{ImageBuffer, Rgba, RgbaImage};
use std::io::Cursor;

/// Maximum image height to prevent OOM on very large pages.
const MAX_IMAGE_HEIGHT: u32 = 16384; // 16K pixels max

/// The 8x16 bitmap font data for ASCII characters 32-126.
/// Each glyph is 8 bytes, one bit per pixel (1=foreground, 0=transparent).
/// Font data is from the classic X11 misc-fixed font (public domain).
const FONT_DATA: &[u8] = include_bytes!("font_8x16.bin");

/// Character width and height in pixels.
const CHAR_W: u32 = 8;
const CHAR_H: u32 = 16;

/// Total number of glyphs in the font table (95 = ASCII 32-126).
const GLYPH_COUNT: usize = 95;

/// Render text content to a PNG image buffer.
///
/// The image has a white background with black monospace text,
/// mimicking a terminal screenshot.
pub fn text_to_png(text: &str, viewport_width: u32) -> Result<Vec<u8>, String> {
    let max_lines = (MAX_IMAGE_HEIGHT / CHAR_H) as usize;
    let lines: Vec<&str> = text.lines().take(max_lines).collect();
    if lines.is_empty() {
        return transparent_png(1, 1);
    }

    // Estimate image dimensions
    // Each line: viewport_width wide, CHAR_H tall
    // Padding: 4px margins
    let margin = 4u32;
    let inner_w = viewport_width.saturating_sub(margin * 2).max(64);
    let chars_per_line = inner_w / CHAR_W;
    let img_h = std::cmp::min(
        lines.len() as u32 * CHAR_H + margin * 2,
        MAX_IMAGE_HEIGHT,
    );
    let img_w = inner_w + margin * 2;

    // White background
    let bg = Rgba([255u8, 255u8, 255u8, 255u8]);
    let fg = Rgba([0u8, 0u8, 0u8, 255u8]);

    let mut img: RgbaImage = ImageBuffer::from_pixel(img_w, img_h, bg);

    for (row, line) in lines.iter().enumerate() {
        let display_line: String = line.chars().take(chars_per_line as usize).collect();
        let x_offset = margin as usize;
        let y_offset = (margin + row as u32 * CHAR_H) as usize;

        for (col, ch) in display_line.chars().enumerate() {
            draw_glyph(&mut img, ch, x_offset + col * CHAR_W as usize, y_offset, fg);
        }
    }

    encode_png(&img)
}

/// Draw a single ASCII glyph onto the image at pixel position (px, py).
fn draw_glyph(img: &mut RgbaImage, ch: char, px: usize, py: usize, color: Rgba<u8>) {
    let code = ch as u32;
    if !(32..=126).contains(&code) {
        return;
    }
    let glyph_idx = (code - 32) as usize;
    if glyph_idx >= GLYPH_COUNT {
        return;
    }

    let offset = glyph_idx * CHAR_H as usize;
    for row in 0..CHAR_H {
        let byte = FONT_DATA[offset + row as usize];
        for col in 0..CHAR_W {
            if byte & (0x80 >> col) != 0 {
                let x = px + col as usize;
                let y = py + row as usize;
                if x < img.width() as usize && y < img.height() as usize {
                    img.get_pixel_mut(x as u32, y as u32).0 = color.0;
                }
            }
        }
    }
}

/// Encode an RGBA image as PNG bytes.
fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut png_bytes = Vec::new();
    let mut cursor = Cursor::new(&mut png_bytes);
    img.write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|e| format!("PNG encoding failed: {}", e))?;
    Ok(png_bytes)
}

/// Create a minimal transparent 1x1 PNG.
fn transparent_png(w: u32, h: u32) -> Result<Vec<u8>, String> {
    let img: RgbaImage = ImageBuffer::from_pixel(w, h, Rgba([0u8, 0u8, 0u8, 0u8]));
    encode_png(&img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_to_png_produces_valid_png() {
        let png = text_to_png("Hello World", 640).expect("encoding should succeed");
        assert!(png.len() > 8);
        // PNG signature: 8 bytes starting with 0x89 0x50 0x4E 0x47
        assert_eq!(&png[0..4], b"\x89PNG");
    }

    #[test]
    fn test_text_to_png_with_newlines() {
        let png = text_to_png("Line 1\nLine 2\nLine 3", 640).expect("encoding should succeed");
        assert!(png.len() > 8);
        assert_eq!(&png[0..4], b"\x89PNG");
    }

    #[test]
    fn test_empty_text_produces_minimal_png() {
        let png = text_to_png("", 640).expect("encoding should succeed");
        assert!(png.len() > 8);
        assert_eq!(&png[0..4], b"\x89PNG");
    }

    #[test]
    fn test_wide_char_truncated() {
        // Characters beyond ASCII should be skipped (no font data)
        let png = text_to_png("Hello🪿World", 640).expect("encoding should succeed");
        assert!(png.len() > 8);
    }

    #[test]
    fn test_oom_protection() {
        // Very large input should be truncated to max lines, not panic
        let big_text: String = (0..200_000).map(|i| format!("Line {i}\n")).collect();
        let png = text_to_png(&big_text, 640).expect("encoding should succeed");
        assert!(png.len() > 8);
    }
}