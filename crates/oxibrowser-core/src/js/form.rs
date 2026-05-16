//! JS code generators for form controls: fill, select, check, upload.
//!
//! All form operations use direct DOM property access + synthetic events.

/// Generate JS to fill an input or textarea.
pub fn js_fill(selector: &str, value: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    let val = serde_json::to_string(value).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return null;
            if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {{
                el.focus();
                var proto = el.tagName === 'TEXTAREA'
                    ? window.HTMLTextAreaElement && window.HTMLTextAreaElement.prototype
                    : window.HTMLInputElement && window.HTMLInputElement.prototype;
                var setter = proto && Object.getOwnPropertyDescriptor(proto, 'value');
                if (setter && setter.set) {{
                    setter.set.call(el, {val});
                }} else {{
                    el.value = {val};
                }}
                el.dispatchEvent(new Event('input', {{ bubbles: true, cancelable: true }}));
                el.dispatchEvent(new Event('change', {{ bubbles: true, cancelable: true }}));
            }} else {{
                el.textContent = {val};
                el.dispatchEvent(new Event('input', {{ bubbles: true, cancelable: true }}));
                el.dispatchEvent(new Event('change', {{ bubbles: true, cancelable: true }}));
            }}
            return el.tagName;
        }})()"#
    )
}

/// Generate JS to select an option in a <select> by value or text.
pub fn js_select_option(selector: &str, value: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    let val = serde_json::to_string(value).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el || el.tagName !== 'SELECT') return null;
            var opts = Array.from(el.options);
            var opt = opts.find(function(o) {{ return o.value === {val} || o.text === {val}; }});
            if (!opt) return null;
            el.value = opt.value;
            el.dispatchEvent(new Event('input', {{ bubbles: true, cancelable: true }}));
            el.dispatchEvent(new Event('change', {{ bubbles: true, cancelable: true }}));
            return el.tagName;
        }})()"#
    )
}

/// Generate JS to check or uncheck a checkbox/radio.
pub fn js_check(selector: &str, checked: bool) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    let checked_js = if checked { "true" } else { "false" };
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return null;
            el.checked = {checked_js};
            el.dispatchEvent(new Event('input', {{ bubbles: true, cancelable: true }}));
            el.dispatchEvent(new Event('change', {{ bubbles: true, cancelable: true }}));
            return el.tagName;
        }})()"#
    )
}

/// Generate JS to upload files to a file input.
pub fn js_upload_file(selector: &str, file_path: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    let path = serde_json::to_string(file_path).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el || el.tagName !== 'INPUT' || el.type !== 'file') return null;
            if (typeof DataTransfer === 'undefined' || typeof File === 'undefined') return null;
            var dt = new DataTransfer();
            var fName = {path}.split('/').pop();
            var f = new File([], fName, {{ type: 'application/octet-stream' }});
            f.__synthetic = true; f.__path = {path};
            dt.items.add(f);
            el.files = dt.files;
            el.dispatchEvent(new Event('input', {{ bubbles: true, cancelable: true }}));
            el.dispatchEvent(new Event('change', {{ bubbles: true, cancelable: true }}));
            return el.tagName;
        }})()"#
    )
}

/// Generate JS to get the value of an input, select, or textContent.
pub fn js_get_value(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return null;
            if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT') {{ return el.value; }}
            return el.textContent;
        }})()"#
    )
}

/// Generate JS to clear an input or textarea.
pub fn js_clear(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return null;
            if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {{
                var proto = el.tagName === 'TEXTAREA'
                    ? window.HTMLTextAreaElement && window.HTMLTextAreaElement.prototype
                    : window.HTMLInputElement && window.HTMLInputElement.prototype;
                var setter = proto && Object.getOwnPropertyDescriptor(proto, 'value');
                if (setter && setter.set) {{
                    setter.set.call(el, '');
                }} else {{
                    el.value = '';
                }}
                el.dispatchEvent(new Event('input', {{ bubbles: true, cancelable: true }}));
                el.dispatchEvent(new Event('change', {{ bubbles: true, cancelable: true }}));
            }}
            return el.tagName;
        }})()"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fill_escapes_quotes() {
        let js = js_fill("#search", "hello \"world\"");
        assert!(js.contains("\\\"world\\\""));
    }

    #[test]
    fn test_select_option_includes_select_tag() {
        let js = js_select_option("#country", "US");
        assert!(js.contains("SELECT"));
        assert!(js.contains("options"));
    }

    #[test]
    fn test_check_sets_checked() {
        let js = js_check("#agree", true);
        assert!(js.contains("checked = true"));
        assert!(js.contains("input"));

        let js2 = js_check("#agree", false);
        assert!(js2.contains("checked = false"));
    }

    #[test]
    fn test_upload_file_creates_datatransfer() {
        let js = js_upload_file("input[type=file]", "/tmp/test.png");
        assert!(js.contains("DataTransfer"));
        assert!(js.contains("File"));
        assert!(js.contains("__synthetic"));
    }

    #[test]
    fn test_get_value_handles_input_and_text_content() {
        let js = js_get_value("#search");
        assert!(js.contains("INPUT"));
        assert!(js.contains("textContent"));
    }

    #[test]
    fn test_clear_uses_input_setter() {
        let js = js_clear("#email");
        // Uses setter.set.call(el, '') when available
        assert!(js.contains("setter.set.call"));
        assert!(js.contains("input"));
    }
}