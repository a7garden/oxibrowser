//! JS code generators for mouse events: hover, drag, scroll, double-click, right-click.
//!
//! All mouse events are dispatched via JavaScript `dispatchEvent()` on DOM elements.
//! No pixel rendering engine is needed — we simulate the browser's event system.

/// Generate JS to hover over an element (mouseover → mouseenter → mousemove).
pub fn js_hover(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return null;
            var rect = el.getBoundingClientRect();
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
            var rect = el.getBoundingClientRect();
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

/// Generate JS to right-click an element (contextmenu event).
pub fn js_right_click(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return null;
            var rect = el.getBoundingClientRect();
            var x = rect.left + rect.width / 2;
            var y = rect.top + rect.height / 2;
            el.dispatchEvent(new MouseEvent('mousedown', {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 2, buttons: 2, detail: 1 }}));
            el.dispatchEvent(new MouseEvent('mouseup', {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 2, buttons: 2, detail: 1 }}));
            el.dispatchEvent(new MouseEvent('contextmenu', {{ bubbles: true, cancelable: true, clientX: x, clientY: y, button: 2, buttons: 2, detail: 1 }}));
            return el.tagName;
        }})()"#
    )
}

/// Generate JS to move the mouse to (x, y) and dispatch mousemove.
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

/// Generate JS to scroll the page by (deltaX, deltaY) pixels.
pub fn js_scroll(delta_x: f64, delta_y: f64) -> String {
    format!(
        r#"(function() {{
            window.scrollBy({delta_x}, {delta_y});
            document.dispatchEvent(new WheelEvent('wheel', {{ bubbles: true, cancelable: true, deltaX: {delta_x}, deltaY: {delta_y}, deltaMode: 0 }}));
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
            if (el) {{ el.scrollIntoView({{ behavior: 'instant', block: {block}, inline: 'nearest' }}); }}
            return el ? el.tagName : null;
        }})()"#
    )
}

/// Generate JS to drag from one element to another.
pub fn js_drag(from_selector: &str, to_selector: &str) -> String {
    let from = serde_json::to_string(from_selector).unwrap_or_default();
    let to = serde_json::to_string(to_selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var fromEl = document.querySelector({from});
            var toEl = document.querySelector({to});
            if (!fromEl || !toEl) return null;
            var fr = fromEl.getBoundingClientRect();
            var tr = toEl.getBoundingClientRect();
            var sx = fr.left + fr.width / 2, sy = fr.top + fr.height / 2;
            var ex = tr.left + tr.width / 2, ey = tr.top + tr.height / 2;
            fromEl.dispatchEvent(new MouseEvent('mousedown', {{ bubbles: true, cancelable: true, clientX: sx, clientY: sy, button: 0, buttons: 1, detail: 1 }}));
            if (fromEl.draggable) {{ fromEl.dispatchEvent(new DragEvent('dragstart', {{ bubbles: true, cancelable: true, clientX: sx, clientY: sy }})); }}
            document.dispatchEvent(new MouseEvent('mousemove', {{ bubbles: true, cancelable: true, clientX: ex, clientY: ey }}));
            if (toEl.draggable !== false) {{ toEl.dispatchEvent(new DragEvent('dragover', {{ bubbles: true, cancelable: true, clientX: ex, clientY: ey }})); }}
            toEl.dispatchEvent(new MouseEvent('mouseup', {{ bubbles: true, cancelable: true, clientX: ex, clientY: ey, button: 0, buttons: 0, detail: 1 }}));
            if (toEl.draggable !== false) {{
                toEl.dispatchEvent(new DragEvent('drop', {{ bubbles: true, cancelable: true, clientX: ex, clientY: ey }}));
                fromEl.dispatchEvent(new DragEvent('dragend', {{ bubbles: true, cancelable: true, clientX: ex, clientY: ey }}));
            }}
            return toEl.tagName;
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
    let code = key_to_code(&key);
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

/// Map a human-readable key name to a DOM `KeyboardEvent.code` string.
pub fn key_to_code(key: &str) -> String {
    match key {
        "Enter" => "Enter".into(),
        "Tab" => "Tab".into(),
        "Escape" | "Esc" => "Escape".into(),
        "Backspace" => "Backspace".into(),
        "Delete" | "Del" => "Delete".into(),
        "ArrowUp" => "ArrowUp".into(),
        "ArrowDown" => "ArrowDown".into(),
        "ArrowLeft" => "ArrowLeft".into(),
        "ArrowRight" => "ArrowRight".into(),
        "Home" => "Home".into(),
        "End" => "End".into(),
        "PageUp" => "PageUp".into(),
        "PageDown" => "PageDown".into(),
        "Space" => "Space".into(),
        "ControlLeft" => "ControlLeft".into(),
        "ControlRight" => "ControlRight".into(),
        "ShiftLeft" => "ShiftLeft".into(),
        "ShiftRight" => "ShiftRight".into(),
        "AltLeft" => "AltLeft".into(),
        "AltRight" => "AltRight".into(),
        "MetaLeft" => "MetaLeft".into(),
        "MetaRight" => "MetaRight".into(),
        "CapsLock" => "CapsLock".into(),
        "F1" => "F1".into(),
        "F2" => "F2".into(),
        "F3" => "F3".into(),
        "F4" => "F4".into(),
        "F5" => "F5".into(),
        "F6" => "F6".into(),
        "F7" => "F7".into(),
        "F8" => "F8".into(),
        "F9" => "F9".into(),
        "F10" => "F10".into(),
        "F11" => "F11".into(),
        "F12" => "F12".into(),
        c if c.len() == 1 => {
            let ch = c.chars().next().unwrap();
            if ch.is_ascii_lowercase() {
                format!("Key{}", ch.to_ascii_uppercase())
            } else if ch.is_ascii_digit() {
                format!("Digit{}", ch)
            } else {
                c.to_string()
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
    fn test_scroll_uses_window_scrollBy() {
        let js = js_scroll(0.0, -300.0);
        assert!(js.contains("window.scrollBy"));
        assert!(js.contains("-300"));
    }

    #[test]
    fn test_parse_key_combo_ctrl_c() {
        let (key, code, mods) = parse_key_combo("Ctrl+C");
        assert_eq!(key, "C");
        assert_eq!(code, "KeyC");
        assert_eq!(mods, 4);
    }

    #[test]
    fn test_parse_key_combo_shift_tab() {
        let (key, code, mods) = parse_key_combo("Shift+Tab");
        assert_eq!(key, "Tab");
        assert_eq!(code, "Tab");
        assert_eq!(mods, 8);
    }

    #[test]
    fn test_key_to_code_arrows() {
        assert_eq!(key_to_code("ArrowUp"), "ArrowUp");
        assert_eq!(key_to_code("ArrowDown"), "ArrowDown");
    }

    #[test]
    fn test_key_to_code_lowercase() {
        assert_eq!(key_to_code("a"), "KeyA");
        assert_eq!(key_to_code("z"), "KeyZ");
    }

    #[test]
    fn test_key_to_code_digits() {
        assert_eq!(key_to_code("0"), "Digit0");
        assert_eq!(key_to_code("9"), "Digit9");
    }
}