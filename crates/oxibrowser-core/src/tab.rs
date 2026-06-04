//! Tab — agent-friendly interactive browsing session.
//!
//! A `Tab` wraps an inner `Session` behind an `Arc<Mutex<Session>>` so that:
//!
//! - All methods take `&self` (no `&mut self`) — callers never manage locks.
//! - `Tab` is `Clone` — multiple agents can share the same tab.
//! - Navigation methods return `BrowseResult` — no chaining needed.
//! - `click`/`type` are built-in — no JS assembly by the consumer.
//!
//! Created via `Browser::new_tab()`.

use crate::browse_result::BrowseResult;
use crate::error::{CoreError, Result};
use crate::event::BrowserEvent;
use crate::js;
use crate::session::Session;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

/// Clone-able, `&self`-only interactive tab for agent use.
///
/// Internally owns an `Arc<Mutex<Session>>`, hiding lock management
/// from the consumer. Created by `Browser::new_tab()`.
pub struct Tab {
    inner: Arc<Mutex<Session>>,
    /// Optional browser tab counter to decrement on close.
    tab_count: Option<Arc<AtomicUsize>>,
    /// Optional event sink to the parent `Browser`'s observer stream.
    ///
    /// When `Some`, navigation/wait/screenshot methods emit `BrowserEvent`s
    /// that observers (e.g. oxi-agent) can subscribe to. When `None` — in
    /// tests or in `Session`-only construction paths — events are silently
    /// dropped.
    event_tx: Option<broadcast::Sender<BrowserEvent>>,
}

impl Clone for Tab {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            tab_count: self.tab_count.clone(),
            event_tx: self.event_tx.clone(),
        }
    }
}

impl Tab {
    /// Create a new Tab wrapping an existing Session.
    /// Used in tests where no browser tab_count tracking or event streaming is needed.
    #[allow(dead_code)]
    pub(crate) fn new(session: Session) -> Self {
        Self {
            inner: Arc::new(Mutex::new(session)),
            tab_count: None,
            event_tx: None,
        }
    }

    /// Create a Tab wired to a parent `Browser`'s tab counter and event stream.
    pub(crate) fn new_with_cleanup_and_events(
        session: Session,
        tab_count: Arc<AtomicUsize>,
        event_tx: broadcast::Sender<BrowserEvent>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(session)),
            tab_count: Some(tab_count),
            event_tx: Some(event_tx),
        }
    }

    /// Emit a `BrowserEvent` if the parent `Browser` wired us up.
    ///
    /// Silently does nothing when the event sink is `None` (e.g. in tests
    /// that build a Tab directly from a Session). On a full observer queue
    /// the event is dropped — observability must never block the hot path.
    fn emit(&self, event: BrowserEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(event);
        }
    }

    /// Count `<script>` blocks referenced by the current page, if loaded.
    ///
    /// Returns 0 when no page is loaded or the DOM has no script resources.
    /// Used for the `js_script_count` field of `BrowserEvent::DocumentReady`.
    fn count_scripts(session: &Session) -> usize {
        match session.page() {
            Some(page) => page
                .root_frame()
                .extract_resource_urls()
                .into_iter()
                .filter(|r| {
                    matches!(
                        r.kind,
                        oxibrowser_webapi::dom::ResourceKind::Script
                    )
                })
                .count(),
            None => 0,
        }
    }

    // -----------------------------------------------------------------------
    // Navigation — all return BrowseResult
    // -----------------------------------------------------------------------

    /// Navigate to a URL.
    pub async fn goto(&self, url: &str) -> Result<BrowseResult> {
        let started = std::time::Instant::now();
        self.emit(BrowserEvent::NavigationStarted {
            url: url.to_string(),
        });

        let mut session = self.inner.lock().await;
        session.navigate(url).await?;
        let result = Self::extract_result(&session);

        self.emit(BrowserEvent::DocumentReady {
            final_url: result.url.clone(),
            title: result.title.clone(),
            status: result.status,
            total_bytes: result.html.len() as u64,
            js_script_count: Self::count_scripts(&session),
            total_duration: started.elapsed(),
        });

        Ok(result)
    }

    /// Go back in history.
    pub async fn back(&self) -> Result<BrowseResult> {
        let mut session = self.inner.lock().await;
        session.go_back().await?;
        Ok(Self::extract_result(&session))
    }

    /// Go forward in history.
    pub async fn forward(&self) -> Result<BrowseResult> {
        let mut session = self.inner.lock().await;
        session.go_forward().await?;
        Ok(Self::extract_result(&session))
    }

    /// Reload the current page.
    pub async fn reload(&self) -> Result<BrowseResult> {
        let mut session = self.inner.lock().await;
        session.reload().await?;
        Ok(Self::extract_result(&session))
    }

    /// POST to a URL and load the response as a page.
    pub async fn post(&self, url: &str, body: &str, content_type: &str) -> Result<BrowseResult> {
        let mut session = self.inner.lock().await;
        session.post(url, body, content_type).await?;
        Ok(Self::extract_result(&session))
    }

    // -----------------------------------------------------------------------
    // Interaction — built on js/input.rs generators
    // -----------------------------------------------------------------------

    /// Click an element matching a CSS selector.
    ///
    /// Dispatches a `click` MouseEvent on the first matching element.
    /// The element's bounding rect is used for coordinates.
    pub async fn click(&self, selector: &str) -> Result<()> {
        let mut session = self.inner.lock().await;

        let sel_json = serde_json::to_string(selector).unwrap_or_default();
        let click_js = format!(
            r#"(function() {{
                var el = document.querySelector({sel_json});
                if (!el) return null;
                var rect = el.getBoundingClientRect
                    ? el.getBoundingClientRect()
                    : {{ left: 0, top: 0, width: 0, height: 0 }};
                var x = rect.left + rect.width / 2;
                var y = rect.top + rect.height / 2;
                el.dispatchEvent(new MouseEvent('click', {{
                    bubbles: true,
                    cancelable: true,
                    clientX: x,
                    clientY: y,
                    button: 0
                }}));
                return el.tagName;
            }})()"#,
        );

        let result = session.evaluate_js(&click_js).await?;
        if result.value.as_ref().is_none_or(|v| v.is_null()) {
            return Err(CoreError::DomError(format!(
                "click: no element matching '{}'",
                selector
            )));
        }
        Ok(())
    }

    /// Type text into an element matching a CSS selector.
    ///
    /// Focuses the element first, then inserts text via `Input.insertText`.
    pub async fn r#type(&self, selector: &str, text: &str) -> Result<()> {
        let mut session = self.inner.lock().await;

        // Focus the target element
        let sel_json = serde_json::to_string(selector).unwrap_or_default();
        let focus_js = format!(
            r#"(function() {{
                var el = document.querySelector({sel_json});
                if (el) {{ el.focus(); return el.tagName; }}
                return null;
            }})()"#,
        );
        let result = session.evaluate_js(&focus_js).await?;
        if result.value.as_ref().is_none_or(|v| v.is_null()) {
            return Err(CoreError::DomError(format!(
                "type: no element matching '{}'",
                selector
            )));
        }

        // Insert text using the input generator
        let insert_js = js::input::js_insert_text(text);
        session.evaluate_js(&insert_js).await?;
        Ok(())
    }

    /// Press a key (dispatches keyDown + keyUp events).
    ///
    /// `key` is a key name like "Enter", "Tab", "Escape", "ArrowDown", etc.
    pub async fn press_key(&self, key: &str) -> Result<()> {
        let mut session = self.inner.lock().await;

        let code = key_to_code(key);
        let down_js =
            js::input::js_dispatch_key_event(key, &code, "keyDown", 0, timestamp_millis());
        session.evaluate_js(&down_js).await?;

        let up_js = js::input::js_dispatch_key_event(key, &code, "keyUp", 0, timestamp_millis());
        session.evaluate_js(&up_js).await?;
        Ok(())
    }

    /// Press a key combo (e.g., "Ctrl+C", "Shift+Tab").
    pub async fn press(&self, combo: &str) -> Result<()> {
        let (key, code, modifiers) = js::mouse::parse_key_combo(combo);
        if key.is_empty() {
            return Err(CoreError::DomError("press: empty key".to_string()));
        }
        let down_js =
            js::input::js_dispatch_key_event(&key, &code, "keyDown", modifiers, timestamp_millis());
        self.eval_js_checked(down_js).await?;
        let up_js =
            js::input::js_dispatch_key_event(&key, &code, "keyUp", modifiers, timestamp_millis());
        self.eval_js_checked(up_js).await?;
        Ok(())
    }

    /// Dispatch a keyDown event (supports modifiers).
    pub async fn key_down(&self, combo: &str) -> Result<()> {
        let (key, code, modifiers) = js::mouse::parse_key_combo(combo);
        if key.is_empty() {
            return Err(CoreError::DomError("key_down: empty key".to_string()));
        }
        let down_js =
            js::input::js_dispatch_key_event(&key, &code, "keyDown", modifiers, timestamp_millis());
        self.eval_js_checked(down_js).await?;
        Ok(())
    }

    /// Dispatch a keyUp event (supports modifiers).
    pub async fn key_up(&self, combo: &str) -> Result<()> {
        let (key, code, modifiers) = js::mouse::parse_key_combo(combo);
        if key.is_empty() {
            return Err(CoreError::DomError("key_up: empty key".to_string()));
        }
        let up_js =
            js::input::js_dispatch_key_event(&key, &code, "keyUp", modifiers, timestamp_millis());
        self.eval_js_checked(up_js).await?;
        Ok(())
    }

    /// Click at viewport coordinates.
    pub async fn click_at(&self, x: f64, y: f64) -> Result<()> {
        let down_js = js::input::js_dispatch_mouse_event(x, y, "mousedown", "left", 1);
        self.eval_js_checked(down_js).await?;
        let up_js = js::input::js_dispatch_mouse_event(x, y, "mouseup", "left", 1);
        self.eval_js_checked(up_js).await?;
        let click_js = js::input::js_dispatch_mouse_event(x, y, "click", "left", 1);
        self.eval_dom_action(click_js, format!("click_at: no element at ({x}, {y})"))
            .await?;
        Ok(())
    }

    /// Double-click an element matching a CSS selector.
    pub async fn double_click(&self, selector: &str) -> Result<()> {
        let js = js::mouse::js_double_click(selector);
        self.eval_dom_action(
            js,
            format!("double_click: no element matching '{selector}'"),
        )
        .await?;
        Ok(())
    }

    /// Right-click an element matching a CSS selector.
    pub async fn right_click(&self, selector: &str) -> Result<()> {
        let js = js::mouse::js_right_click(selector);
        self.eval_dom_action(js, format!("right_click: no element matching '{selector}'"))
            .await?;
        Ok(())
    }

    /// Hover over an element matching a CSS selector.
    pub async fn hover(&self, selector: &str) -> Result<()> {
        let js = js::mouse::js_hover(selector);
        self.eval_dom_action(js, format!("hover: no element matching '{selector}'"))
            .await?;
        Ok(())
    }

    /// Move mouse to viewport coordinates.
    pub async fn move_mouse(&self, x: f64, y: f64) -> Result<()> {
        let js = js::mouse::js_move_mouse(x, y);
        self.eval_dom_action(js, format!("move_mouse: no element at ({x}, {y})"))
            .await?;
        Ok(())
    }

    /// Scroll by (delta_x, delta_y) pixels.
    pub async fn scroll(&self, delta_x: f64, delta_y: f64) -> Result<()> {
        let js = js::mouse::js_scroll(delta_x, delta_y);
        self.eval_dom_action(js, "scroll failed".to_string())
            .await?;
        Ok(())
    }

    /// Scroll the first matching element into view.
    pub async fn scroll_into_view(&self, selector: &str, center: bool) -> Result<()> {
        let js = js::mouse::js_scroll_into_view(selector, center);
        self.eval_dom_action(
            js,
            format!("scroll_into_view: no element matching '{selector}'"),
        )
        .await?;
        Ok(())
    }

    /// Drag from one selector to another.
    pub async fn drag(&self, from_selector: &str, to_selector: &str) -> Result<()> {
        let js = js::mouse::js_drag(from_selector, to_selector);
        self.eval_dom_action(
            js,
            format!(
                "drag: no element matching '{}' or '{}'",
                from_selector, to_selector
            ),
        )
        .await?;
        Ok(())
    }

    /// Fill an input/textarea or contentEditable with a value.
    pub async fn fill(&self, selector: &str, value: &str) -> Result<()> {
        let js = js::form::js_fill(selector, value);
        self.eval_dom_action(js, format!("fill: no element matching '{selector}'"))
            .await?;
        Ok(())
    }

    /// Select an option by value or text.
    pub async fn select_option(&self, selector: &str, value: &str) -> Result<()> {
        let js = js::form::js_select_option(selector, value);
        self.eval_dom_action(
            js,
            format!("select_option: no element matching '{selector}'"),
        )
        .await?;
        Ok(())
    }

    /// Check a checkbox or radio input.
    pub async fn check(&self, selector: &str) -> Result<()> {
        let js = js::form::js_check(selector, true);
        self.eval_dom_action(js, format!("check: no element matching '{selector}'"))
            .await?;
        Ok(())
    }

    /// Uncheck a checkbox or radio input.
    pub async fn uncheck(&self, selector: &str) -> Result<()> {
        let js = js::form::js_check(selector, false);
        self.eval_dom_action(js, format!("uncheck: no element matching '{selector}'"))
            .await?;
        Ok(())
    }

    /// Upload a file (synthetic) to an <input type="file"> element.
    pub async fn upload_file(&self, selector: &str, file_path: &str) -> Result<()> {
        let js = js::form::js_upload_file(selector, file_path);
        self.eval_dom_action(js, format!("upload_file: no element matching '{selector}'"))
            .await?;
        Ok(())
    }

    /// Clear an input or textarea value.
    pub async fn clear_input(&self, selector: &str) -> Result<()> {
        let js = js::form::js_clear(selector);
        self.eval_dom_action(js, format!("clear_input: no element matching '{selector}'"))
            .await?;
        Ok(())
    }

    /// Get the current value/textContent for the first matching element.
    pub async fn get_value(&self, selector: &str) -> Result<String> {
        let js = js::form::js_get_value(selector);
        let value = self
            .eval_dom_action(js, format!("get_value: no element matching '{selector}'"))
            .await?;
        match value {
            Value::String(s) => Ok(s),
            Value::Null => Ok(String::new()),
            other => Ok(other.to_string()),
        }
    }

    /// Get an attribute value from the first matching element.
    pub async fn query_attr(&self, selector: &str, attr: &str) -> Result<Option<String>> {
        let sel_json = serde_json::to_string(selector).unwrap_or_default();
        let attr_json = serde_json::to_string(attr).unwrap_or_default();
        let js = format!(
            r#"(function() {{
                var el = document.querySelector({sel_json});
                if (!el) return {{ found: false }};
                return {{ found: true, value: el.getAttribute({attr_json}) }};
            }})()"#,
        );
        let value = self.eval_js_checked(js).await?;
        if let Value::Object(map) = value {
            if map.get("found").and_then(|v| v.as_bool()) == Some(true) {
                let attr_val = map
                    .get("value")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                return Ok(attr_val);
            }
        }
        Err(CoreError::DomError(format!(
            "query_attr: no element matching '{selector}'"
        )))
    }

    // -----------------------------------------------------------------------
    // Content extraction
    // -----------------------------------------------------------------------

    /// Get the current page content as a `BrowseResult`.
    ///
    /// Does not navigate — just extracts from the currently loaded page.
    pub async fn content(&self) -> Result<BrowseResult> {
        let session = self.inner.lock().await;
        Ok(Self::extract_result(&session))
    }

    /// Get text content of all elements matching a CSS selector.
    pub async fn query_all(&self, selector: &str) -> Result<Vec<String>> {
        let mut session = self.inner.lock().await;

        let sel_json = serde_json::to_string(selector).unwrap_or_default();
        let js = format!(
            r#"(function() {{
                var els = document.querySelectorAll({sel_json});
                return Array.from(els).map(function(el) {{ return el.textContent; }});
            }})()"#,
        );

        let result = session.evaluate_js(&js).await?;
        Ok(parse_js_string_array(result.value.as_ref()))
    }

    /// Evaluate JavaScript (does not await Promises).
    pub async fn evaluate(&self, expression: &str) -> Result<Value> {
        let mut session = self.inner.lock().await;
        let result = session.evaluate_js(expression).await?;
        match result.exception {
            Some(e) => Err(CoreError::JsError(e)),
            None => Ok(result.value.unwrap_or(Value::Null)),
        }
    }

    /// Evaluate JavaScript, awaiting Promise resolution.
    pub async fn evaluate_await(&self, expression: &str) -> Result<Value> {
        let mut session = self.inner.lock().await;
        let result = session.evaluate_js_with_await(expression, true).await?;
        match result.exception {
            Some(e) => Err(CoreError::JsError(e)),
            None => Ok(result.value.unwrap_or(Value::Null)),
        }
    }

    // -----------------------------------------------------------------------
    // Waiting
    // -----------------------------------------------------------------------

    /// Wait until a CSS selector matches at least one element.
    ///
    /// Polls every 50ms. Returns error on timeout.
    pub async fn wait_for(&self, selector: &str, timeout_ms: u64) -> Result<()> {
        self.emit(BrowserEvent::WaitingForSelector {
            selector: selector.to_string(),
            timeout_ms,
        });

        let start = std::time::Instant::now();
        let deadline = start + std::time::Duration::from_millis(timeout_ms);

        loop {
            {
                let session = self.inner.lock().await;
                if let Some(page) = session.page() {
                    if page.root_frame().query_selector(selector).is_some() {
                        return Ok(());
                    }
                }
            }
            // release the lock before sleeping
            if std::time::Instant::now() >= deadline {
                return Err(CoreError::Timeout(format!(
                    "wait_for('{}') timed out after {}ms",
                    selector, timeout_ms
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    // -----------------------------------------------------------------------
    // Sub-resources
    // -----------------------------------------------------------------------

    /// Load sub-resources (JS, CSS, images) referenced by the current page.
    ///
    /// Returns the number of resources successfully loaded.
    pub async fn load_resources(&self) -> Result<usize> {
        let mut session = self.inner.lock().await;
        Ok(session.load_sub_resources().await)
    }

    // -----------------------------------------------------------------------
    // Screenshot
    // -----------------------------------------------------------------------

    /// Render the current page as a PNG screenshot (text-based bitmap font).
    pub async fn screenshot(&self, width: u32) -> Result<Vec<u8>> {
        let started = std::time::Instant::now();
        let session = self.inner.lock().await;
        let png = match session.page() {
            Some(page) => page.to_screenshot_png(width)?,
            None => return Err(CoreError::PageNotLoaded),
        };

        self.emit(BrowserEvent::ScreenshotCaptured {
            bytes: png.len(),
            viewport_width: width,
            duration: started.elapsed(),
        });
        Ok(png)
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Close this tab.
    pub async fn close(&self) -> Result<()> {
        let mut session = self.inner.lock().await;
        let result = session.close().await;
        if result.is_ok() {
            if let Some(ref counter) = self.tab_count {
                counter.fetch_sub(1, Ordering::Relaxed);
            }
        }
        result
    }

    /// Whether this tab has been closed.
    pub fn is_closed(&self) -> bool {
        // Non-blocking check: try_lock succeeds ⇒ check is_closed
        match self.inner.try_lock() {
            Ok(session) => session.is_closed(),
            Err(_) => false, // locked ⇒ still alive
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Evaluate JS and surface exceptions as CoreError::JsError.
    async fn eval_js_checked(&self, js_code: String) -> Result<Value> {
        let mut session = self.inner.lock().await;
        let result = session.evaluate_js(&js_code).await?;
        if let Some(exception) = result.exception {
            return Err(CoreError::JsError(exception));
        }
        Ok(result.value.unwrap_or(Value::Null))
    }

    /// Evaluate JS and ensure the result is non-null (DOM element found).
    async fn eval_dom_action(&self, js_code: String, error: String) -> Result<Value> {
        let value = self.eval_js_checked(js_code).await?;
        if value.is_null() {
            return Err(CoreError::DomError(error));
        }
        Ok(value)
    }

    /// Extract BrowseResult from a Session's current page.
    fn extract_result(session: &Session) -> BrowseResult {
        match session.page() {
            Some(page) => BrowseResult::from_page(page),
            None => BrowseResult::empty(),
        }
    }
}

// -----------------------------------------------------------------------
// Key name → code mapping (for press_key)
// -----------------------------------------------------------------------

/// Map a human-readable key name to a DOM `KeyboardEvent.code` string.
fn key_to_code(key: &str) -> String {
    // Common special keys — single-char keys use "KeyX" pattern
    match key {
        "Enter" => "Enter".to_string(),
        "Tab" => "Tab".to_string(),
        "Escape" => "Escape".to_string(),
        "Backspace" => "Backspace".to_string(),
        "Delete" => "Delete".to_string(),
        "ArrowUp" => "ArrowUp".to_string(),
        "ArrowDown" => "ArrowDown".to_string(),
        "ArrowLeft" => "ArrowLeft".to_string(),
        "ArrowRight" => "ArrowRight".to_string(),
        "Home" => "Home".to_string(),
        "End" => "End".to_string(),
        "PageUp" => "PageUp".to_string(),
        "PageDown" => "PageDown".to_string(),
        "Space" => "Space".to_string(),
        "Control" | "ControlLeft" => "ControlLeft".to_string(),
        "ControlRight" => "ControlRight".to_string(),
        "Shift" | "ShiftLeft" => "ShiftLeft".to_string(),
        "ShiftRight" => "ShiftRight".to_string(),
        "Alt" | "AltLeft" => "AltLeft".to_string(),
        "AltRight" => "AltRight".to_string(),
        "Meta" | "MetaLeft" => "MetaLeft".to_string(),
        "MetaRight" => "MetaRight".to_string(),
        "CapsLock" => "CapsLock".to_string(),
        "F1" => "F1".to_string(),
        "F2" => "F2".to_string(),
        "F3" => "F3".to_string(),
        "F4" => "F4".to_string(),
        "F5" => "F5".to_string(),
        "F6" => "F6".to_string(),
        "F7" => "F7".to_string(),
        "F8" => "F8".to_string(),
        "F9" => "F9".to_string(),
        "F10" => "F10".to_string(),
        "F11" => "F11".to_string(),
        "F12" => "F12".to_string(),
        c if c.len() == 1 && c.chars().next().is_some_and(|ch| ch.is_ascii_lowercase()) => {
            format!("Key{}", c.to_ascii_uppercase())
        }
        c if c.len() == 1 && c.chars().next().is_some_and(|ch| ch.is_ascii_digit()) => {
            format!("Digit{}", c)
        }
        _ => key.to_string(),
    }
}

/// Current timestamp in fractional milliseconds (for KeyboardEvent).
fn timestamp_millis() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}

/// Parse a `serde_json::Value` (expected array of strings) into `Vec<String>`.
fn parse_js_string_array(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::BrowserId;
    use crate::config::BrowserConfig;
    use crate::network::cookie::CookieJar;
    use crate::network::HttpClient;
    use crate::page::Page;
    use parking_lot::RwLock;

    /// Helper: create a Tab with a session loaded with an HTML page.
    async fn tab_with_html(html: &str) -> Tab {
        let config = BrowserConfig::headless();
        let cookie_jar = Arc::new(RwLock::new(CookieJar::new()));
        let http_client = Arc::new(HttpClient::new(&config, cookie_jar.clone()).unwrap());
        let mut session = Session::new(BrowserId::next(), config, http_client, cookie_jar)
            .await
            .unwrap();

        // Load HTML directly via navigate-style logic
        let url = url::Url::parse("https://test.local/page").unwrap();
        let page = Page::from_html(url, html, 200, "text/html".to_string())
            .await
            .unwrap();
        session.inject_dom_snapshot_for_test(page);
        Tab::new(session)
    }

    #[tokio::test]
    async fn test_tab_content_extracts_browse_result() {
        let html = "<!DOCTYPE html><html><head><title>Test Title</title></head>\
                     <body><p>Hello World</p></body></html>";
        let tab = tab_with_html(html).await;

        let result = tab.content().await.unwrap();
        assert_eq!(result.url, "https://test.local/page");
        assert_eq!(result.title, "Test Title");
        assert_eq!(result.status, 200);
        assert!(result.markdown.contains("Hello World"));
        assert!(result.html.contains("<p>Hello World</p>"));
    }

    #[tokio::test]
    async fn test_tab_clone_shared_state() {
        let html = "<!DOCTYPE html><html><head><title>Shared</title></head>\
                     <body><p>Content</p></body></html>";
        let tab = tab_with_html(html).await;
        let tab2 = tab.clone();

        let r1 = tab.content().await.unwrap();
        let r2 = tab2.content().await.unwrap();
        assert_eq!(r1.title, r2.title);
        assert_eq!(r1.url, r2.url);
    }

    #[tokio::test]
    async fn test_tab_query_all() {
        let html = "<!DOCTYPE html><html><body>\
                     <ul>\
                       <li class=\"item\">First</li>\
                       <li class=\"item\">Second</li>\
                       <li class=\"item\">Third</li>\
                     </ul>\
                     </body></html>";
        let tab = tab_with_html(html).await;

        let items = tab.query_all(".item").await.unwrap();
        assert_eq!(
            items.len(),
            3,
            "should find 3 .item elements, got: {items:?}"
        );
        // textContent includes all child text
        assert!(
            items.iter().any(|t| t.contains("First")),
            "should contain First: {items:?}"
        );
        assert!(
            items.iter().any(|t| t.contains("Second")),
            "should contain Second: {items:?}"
        );
        assert!(
            items.iter().any(|t| t.contains("Third")),
            "should contain Third: {items:?}"
        );
    }

    #[tokio::test]
    async fn test_tab_query_all_no_match() {
        let html = "<!DOCTYPE html><html><body><p>Hello</p></body></html>";
        let tab = tab_with_html(html).await;

        let items = tab.query_all(".nonexistent").await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_tab_evaluate_js() {
        let html = "<!DOCTYPE html><html><body><p>JS Test</p></body></html>";
        let tab = tab_with_html(html).await;

        let result = tab.evaluate("1 + 2").await.unwrap();
        assert_eq!(result, serde_json::json!(3));
    }

    #[tokio::test]
    async fn test_tab_evaluate_json_roundtrip() {
        let html = "<!DOCTYPE html><html><body></body></html>";
        let tab = tab_with_html(html).await;

        let result = tab
            .evaluate("JSON.stringify({key: 'value', num: 42})")
            .await
            .unwrap();
        // Result is a JSON string
        let parsed: serde_json::Value = serde_json::from_str(result.as_str().unwrap()).unwrap();
        assert_eq!(parsed["key"], "value");
        assert_eq!(parsed["num"], 42);
    }

    #[tokio::test]
    async fn test_tab_evaluate_js_error() {
        let html = "<!DOCTYPE html><html><body></body></html>";
        let tab = tab_with_html(html).await;

        let result = tab.evaluate("throw new Error('boom')").await;
        assert!(result.is_err());
        match result {
            Err(CoreError::JsError(msg)) => {
                assert!(msg.contains("boom"), "error should mention 'boom': {msg}");
            }
            Err(e) => panic!("wrong error type: {e:?}"),
            Ok(_) => panic!("should have failed"),
        }
    }

    #[tokio::test]
    async fn test_tab_screenshot() {
        let html = "<!DOCTYPE html><html><head><title>Shot</title></head>\
                     <body><p>Screenshot test</p></body></html>";
        let tab = tab_with_html(html).await;

        let png = tab.screenshot(800).await.unwrap();
        // PNG magic header
        assert!(png.len() > 8);
        assert_eq!(&png[0..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    #[tokio::test]
    async fn test_tab_close() {
        let html = "<!DOCTYPE html><html><body><p>Close me</p></body></html>";
        let tab = tab_with_html(html).await;
        assert!(!tab.is_closed());

        tab.close().await.unwrap();
        assert!(tab.is_closed());
    }

    #[tokio::test]
    async fn test_tab_close_twice_no_panic() {
        let html = "<!DOCTYPE html><html><body></body></html>";
        let tab = tab_with_html(html).await;

        tab.close().await.unwrap();
        tab.close().await.unwrap(); // Should not panic
        assert!(tab.is_closed());
    }

    #[tokio::test]
    async fn test_tab_without_event_sink_silently_drops() {
        // Tabs built via Tab::new() (test path) have no event_tx;
        // emit() should silently no-op.
        use crate::event::BrowserEvent;
        let html = "<!DOCTYPE html><html><body><p>Hi</p></body></html>";
        let tab = tab_with_html(html).await;
        // Should not panic.
        tab.emit(BrowserEvent::NavigationStarted {
            url: "https://test".into(),
        });
    }

    #[tokio::test]
    async fn test_tab_with_event_sink_emits_on_screenshot() {
        use crate::browser::Browser;
        use crate::config::BrowserConfig;
        use crate::event::BrowserEvent;

        let browser = Browser::new(BrowserConfig::headless()).await.unwrap();
        let mut rx = browser.subscribe_events();
        let tab = browser.new_tab().await.unwrap();

        // The Tab holds a clone of browser's event_tx. Emit through the Tab
        // should reach this subscriber.
        tab.emit(BrowserEvent::ScreenshotCaptured {
            bytes: 1024,
            viewport_width: 800,
            duration: std::time::Duration::from_millis(10),
        });

        let event = rx.try_recv().expect("subscriber should receive event");
        match event {
            BrowserEvent::ScreenshotCaptured {
                bytes,
                viewport_width,
                ..
            } => {
                assert_eq!(bytes, 1024);
                assert_eq!(viewport_width, 800);
            }
            other => panic!("expected ScreenshotCaptured, got {other:?}"),
        }
    }

    #[test]
    fn test_key_to_code_special_keys() {
        assert_eq!(key_to_code("Enter"), "Enter");
        assert_eq!(key_to_code("Tab"), "Tab");
        assert_eq!(key_to_code("Escape"), "Escape");
        assert_eq!(key_to_code("ArrowDown"), "ArrowDown");
        assert_eq!(key_to_code("Space"), "Space");
        assert_eq!(key_to_code("F5"), "F5");
    }

    #[test]
    fn test_key_to_code_letters() {
        assert_eq!(key_to_code("a"), "KeyA");
        assert_eq!(key_to_code("z"), "KeyZ");
    }

    #[test]
    fn test_key_to_code_digits() {
        assert_eq!(key_to_code("0"), "Digit0");
        assert_eq!(key_to_code("9"), "Digit9");
    }

    #[test]
    fn test_key_to_code_modifiers() {
        assert_eq!(key_to_code("Control"), "ControlLeft");
        assert_eq!(key_to_code("Shift"), "ShiftLeft");
        assert_eq!(key_to_code("Alt"), "AltLeft");
        assert_eq!(key_to_code("Meta"), "MetaLeft");
        assert_eq!(key_to_code("ControlRight"), "ControlRight");
        assert_eq!(key_to_code("ShiftRight"), "ShiftRight");
    }

    #[test]
    fn test_parse_js_string_array() {
        let v = serde_json::json!(["hello", "world"]);
        let result = parse_js_string_array(Some(&v));
        assert_eq!(result, vec!["hello", "world"]);
    }

    #[test]
    fn test_parse_js_string_array_empty() {
        assert_eq!(parse_js_string_array(None), Vec::<String>::new());
        assert_eq!(
            parse_js_string_array(Some(&serde_json::json!(null))),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_js_string_array(Some(&serde_json::json!([]))),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_parse_js_string_array_skips_non_strings() {
        let v = serde_json::json!(["ok", 42, true, "also ok"]);
        let result = parse_js_string_array(Some(&v));
        assert_eq!(result, vec!["ok", "also ok"]);
    }
}
