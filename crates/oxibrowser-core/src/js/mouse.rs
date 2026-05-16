//! JS code generators for mouse events: hover, drag, scroll, double-click, right-click.
//!
//! All mouse events are dispatched via JavaScript `dispatchEvent()` on DOM elements.

/// Generate JS to hover over an element (mouseover → mouseenter → mousemove).
pub fn js_hover(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return null;
            var rect = el.getBoundingClientRect
                ? el.getBoundingClientRect()
                : {{ left: 0, top: 0, width: 0, height: 0 }};
            var x = rect.left + rect.width / 2;
            var y = rect.top + rect.height / 2;
            el.dispatchEvent(new MouseEvent('mouseover', {{ bubbles: true, cancelable: true, clientX: x, clientY: y }}));
            el.dispatchEvent(new MouseEvent('mouseenter', {{ bubbles: false, cancelable: true, clientX: x, clientY: y }}));
            el.dispatchEvent(new MouseEvent('mousemove', {{ bubbles: true, cancelable: true, clientX: x, clientY: y }}));
            return el.tagName;
        }})()"#
    )
}

/// Generate JS to double-click an element.
pub fn js_double_click(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return null;
            var rect = el.getBoundingClientRect
                ? el.getBoundingClientRect()
                : {{ left: 0, top: 0, width: 0, height: 0 }};
            var x = rect.left + rect.width / 2;
            var y = rect.top + rect.height / 2;
            el.dispatchEvent(new MouseEvent('mousedown', {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 0, buttons: 1, detail: 1 }}));
            el.dispatchEvent(new MouseEvent('click', {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 0, buttons: 1, detail: 1 }}));
            el.dispatchEvent(new MouseEvent('mousedown', {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 0, buttons: 1, detail: 2 }}));
            el.dispatchEvent(new MouseEvent('dblclick', {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 0, buttons: 1, detail: 2 }}));
            el.dispatchEvent(new MouseEvent('mouseup', {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 0, buttons: 1, detail: 2 }}));
            return el.tagName;
        }})()"#
    )
}

/// Generate JS to right-click an element.
pub fn js_right_click(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return null;
            var rect = el.getBoundingClientRect
                ? el.getBoundingClientRect()
                : {{ left: 0, top: 0, width: 0, height: 0 }};
            var x = rect.left + rect.width / 2;
            var y = rect.top + rect.height / 2;
            el.dispatchEvent(new MouseEvent('mousedown', {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 2, buttons: 2, detail: 1 }}));
            el.dispatchEvent(new MouseEvent('mouseup', {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 2, buttons: 2, detail: 1 }}));
            el.dispatchEvent(new MouseEvent('contextmenu', {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 2, buttons: 2, detail: 1 }}));
            return el.tagName;
        }})()"#
    )
}

/// Generate JS to move the mouse to (x, y).
pub fn js_move_mouse(x: f64, y: f64) -> String {
    format!(
        r#"(function() {{
            var el = document.elementFromPoint({x}, {y});
            if (el) {{
                el.dispatchEvent(new MouseEvent('mousemove', {{ bubbles: true, cancelable: true, clientX: {x}, clientY: {y}, button: 0, buttons: 0 }}));
                document.dispatchEvent(new MouseEvent('mousemove', {{ bubbles: true, cancelable: true, clientX: {x}, clientY: {y}, button: 0, buttons: 0 }}));
            }}
            return el ? el.tagName : null;
        }})()"#
    )
}

/// Generate JS to scroll by (deltaX, deltaY) pixels.
pub fn js_scroll(delta_x: f64, delta_y: f64) -> String {
    format!(
        r#"(function() {{
            window.scrollBy({delta_x}, {delta_y});
            if (typeof WheelEvent !== 'undefined') {{
                document.dispatchEvent(new WheelEvent('wheel', {{ bubbles: true, cancelable: true, deltaX: {delta_x}, deltaY: {delta_y}, deltaMode: 0 }}));
            }}
            return true;
        }})()"#
    )
}

/// Generate JS to scroll an element into view.
pub fn js_scroll_into_view(selector: &str, center: bool) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    let block = if center { "'center'" } else { "'nearest'" };
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (el && typeof el.scrollIntoView === 'function') {{
                el.scrollIntoView({{ behavior: 'instant', block: {block}, inline: 'nearest' }});
            }}
            return el ? el.tagName : null;
        }})()"#
    )
}

/// Generate JS to drag from one element to another.
pub fn js_drag(from_sel: &str, to_sel: &str) -> String {
    let from = serde_json::to_string(from_sel).unwrap_or_default();
    let to = serde_json::to_string(to_sel).unwrap_or_default();
    format!(
        r#"(function() {{
            var fe = document.querySelector({from});
            var te = document.querySelector({to});
            if (!fe || !te) return null;
            var fr = fe.getBoundingClientRect
                ? fe.getBoundingClientRect()
                : {{ left: 0, top: 0, width: 0, height: 0 }};
            var tr = te.getBoundingClientRect
                ? te.getBoundingClientRect()
                : {{ left: 0, top: 0, width: 0, height: 0 }};
            var sx = fr.left + fr.width / 2, sy = fr.top + fr.height / 2;
            var ex = tr.left + tr.width / 2, ey = tr.top + tr.height / 2;
            var hasDrag = typeof DragEvent !== 'undefined';
            fe.dispatchEvent(new MouseEvent('mousedown', {{ bubbles: true, cancelable: true, clientX: sx, clientY: sy, button: 0, buttons: 1, detail: 1 }}));
            if (hasDrag && fe.draggable) {{ fe.dispatchEvent(new DragEvent('dragstart', {{ bubbles: true, cancelable: true, clientX: sx, clientY: sy }})); }}
            document.dispatchEvent(new MouseEvent('mousemove', {{ bubbles: true, cancelable: true, clientX: ex, clientY: ey }}));
            if (hasDrag && te.draggable !== false) {{ te.dispatchEvent(new DragEvent('dragover', {{ bubbles: true, cancelable: true, clientX: ex, clientY: ey }})); }}
            te.dispatchEvent(new MouseEvent('mouseup', {{ bubbles: true, cancelable: true, clientX: ex, clientY: ey, button: 0, buttons: 0, detail: 1 }}));
            if (hasDrag && te.draggable !== false) {{
                te.dispatchEvent(new DragEvent('drop', {{ bubbles: true, cancelable: true, clientX: ex, clientY: ey }}));
                fe.dispatchEvent(new DragEvent('dragend', {{ bubbles: true, cancelable: true, clientX: ex, clientY: ey }}));
            }}
            return te.tagName;
        }})()"#
    )
}

/// Parse a key combo string (e.g., "Ctrl+C") into (key, code, modifiers_bitmask).
pub fn parse_key_combo(combo: &str) -> (String, String, u32) {
    let parts: Vec<&str> = combo.split('+').collect();
    if parts.is_empty() {
        return (String::new(), String::new(), 0);
    }
    let key = parts.last().unwrap().to_string();
    // Normalize key to lowercase for code lookup; preserve raw key for KeyboardEvent.key
    let code = key_to_code(&key.to_lowercase());
    let mut modifiers = 0u32;
    for part in &parts[..parts.len().saturating_sub(1)] {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= 4,
            "shift" => modifiers |= 8,
            "alt" | "option" => modifiers |= 2,
            "meta" | "cmd" | "command" | "super" => modifiers |= 16,
            _ => {}
        }
    }
    (key, code, modifiers)
}

/// Map a key name to a DOM KeyboardEvent.code string.
/// Handles both lowercase (e.g., "enter", "arrowup") and mixed-case (e.g., "Enter", "ArrowUp").
pub fn key_to_code(key: &str) -> String {
    let k = key.to_lowercase();
    match k.as_str() {
        "enter" => "Enter".into(),
        "tab" => "Tab".into(),
        "escape" | "esc" => "Escape".into(),
        "backspace" => "Backspace".into(),
        "delete" | "del" => "Delete".into(),
        "arrowup" => "ArrowUp".into(),
        "arrowdown" => "ArrowDown".into(),
        "arrowleft" => "ArrowLeft".into(),
        "arrowright" => "ArrowRight".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        "space" => "Space".into(),
        "control" => "ControlLeft".into(),
        "controlleft" => "ControlLeft".into(),
        "controlright" => "ControlRight".into(),
        "shift" => "ShiftLeft".into(),
        "shiftleft" => "ShiftLeft".into(),
        "shiftright" => "ShiftRight".into(),
        "alt" => "AltLeft".into(),
        "altleft" => "AltLeft".into(),
        "altright" => "AltRight".into(),
        "meta" => "MetaLeft".into(),
        "metaleft" => "MetaLeft".into(),
        "metaright" => "MetaRight".into(),
        "capslock" => "CapsLock".into(),
        "f1" => "F1".into(),
        "f2" => "F2".into(),
        "f3" => "F3".into(),
        "f4" => "F4".into(),
        "f5" => "F5".into(),
        "f6" => "F6".into(),
        "f7" => "F7".into(),
        "f8" => "F8".into(),
        "f9" => "F9".into(),
        "f10" => "F10".into(),
        "f11" => "F11".into(),
        "f12" => "F12".into(),
        c if c.len() == 1 => {
            let ch = c.chars().next().unwrap();
            if ch.is_ascii_alphabetic() {
                format!("Key{}", ch.to_ascii_uppercase())
            } else if ch.is_ascii_digit() {
                format!("Digit{}", ch)
            } else {
                key.to_string()
            }
        }
        _ => key.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hover_generates_mouse_events() {
        let js = js_hover(".btn");
        assert!(js.contains("mouseover"));
        assert!(js.contains("mouseenter"));
        assert!(js.contains("mousemove"));
    }

    #[test]
    fn test_double_click_includes_dblclick() {
        let js = js_double_click(".item");
        assert!(js.contains("dblclick"));
        assert!(js.contains("detail: 2"));
    }

    #[test]
    fn test_right_click_includes_contextmenu() {
        let js = js_right_click(".menu");
        assert!(js.contains("contextmenu"));
        assert!(js.contains("button: 2"));
    }

    #[test]
    fn test_scroll_uses_window_scroll_by() {
        let js = js_scroll(0.0, -300.0);
        assert!(js.contains("window.scrollBy"));
        assert!(js.contains("-300"));
    }

    #[test]
    fn test_parse_key_combo_ctrl_c() {
        let (key, code, mods) = parse_key_combo("Ctrl+C");
        assert_eq!(key, "C"); // raw key preserved
        assert_eq!(code, "KeyC"); // keyboard code
        assert_eq!(mods, 4); // Ctrl
    }

    #[test]
    fn test_parse_key_combo_shift_tab() {
        let (key, code, mods) = parse_key_combo("Shift+Tab");
        assert_eq!(key, "Tab"); // raw key preserved
        assert_eq!(code, "Tab"); // keyboard code
        assert_eq!(mods, 8); // Shift
    }

    #[test]
    fn test_parse_key_combo_meta_shift_a() {
        let (key, code, mods) = parse_key_combo("Meta+Shift+A");
        assert_eq!(key, "A");
        assert_eq!(code, "KeyA");
        assert_eq!(mods, 8 | 16); // Shift + Meta
    }

    #[test]
    fn test_key_to_code_arrows() {
        assert_eq!(key_to_code("ArrowUp"), "ArrowUp");
        assert_eq!(key_to_code("arrowup"), "ArrowUp"); // lowercase too
        assert_eq!(key_to_code("ArrowDown"), "ArrowDown");
        assert_eq!(key_to_code("ArrowLeft"), "ArrowLeft");
        assert_eq!(key_to_code("ArrowRight"), "ArrowRight");
    }

    #[test]
    fn test_key_to_code_lowercase() {
        assert_eq!(key_to_code("a"), "KeyA");
        assert_eq!(key_to_code("z"), "KeyZ");
    }

    #[test]
    fn test_key_to_code_uppercase() {
        assert_eq!(key_to_code("A"), "KeyA");
        assert_eq!(key_to_code("C"), "KeyC");
    }

    #[test]
    fn test_key_to_code_digits() {
        assert_eq!(key_to_code("0"), "Digit0");
        assert_eq!(key_to_code("9"), "Digit9");
    }

    #[test]
    fn test_key_to_code_special() {
        assert_eq!(key_to_code("Enter"), "Enter");
        assert_eq!(key_to_code("Tab"), "Tab");
        assert_eq!(key_to_code("Escape"), "Escape");
        assert_eq!(key_to_code("Space"), "Space");
        assert_eq!(key_to_code("F5"), "F5");
        assert_eq!(key_to_code("Control"), "ControlLeft");
        assert_eq!(key_to_code("Shift"), "ShiftLeft");
        assert_eq!(key_to_code("Alt"), "AltLeft");
        assert_eq!(key_to_code("Meta"), "MetaLeft");
        assert_eq!(key_to_code("AltRight"), "AltRight");
        assert_eq!(key_to_code("altright"), "AltRight");
    }
}