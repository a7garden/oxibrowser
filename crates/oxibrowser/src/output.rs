//! CLI output utilities — standard JSON response wrapper, truncation, field filtering.

use serde_json::Value;

/// Exit codes for agent-consumable error classification.
pub mod exit_code {
    /// Success (including truncated output).
    pub const OK: i32 = 0;
    /// Runtime error: DOM not found, JS error, element not found.
    pub const RUNTIME: i32 = 1;
    /// Input validation failure: bad URL, control chars, path traversal.
    pub const INPUT: i32 = 2;
    /// Timeout.
    pub const TIMEOUT: i32 = 3;
    /// Network error: DNS, connection refused, HTTP 4xx/5xx.
    pub const NETWORK: i32 = 4;
}

/// Standard CLI JSON response wrapper.
///
/// Every JSON output from oxibrowser uses this structure so agents can
/// parse uniformly: check `ok`, then read `data` or `error`.
#[derive(Debug, serde::Serialize)]
pub struct CliResponse {
    /// Success flag. Always present.
    pub ok: bool,
    /// Result data on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Human-readable error message on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Machine-readable error code on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Metadata (timing, tab info).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Response metadata.
#[derive(Debug, serde::Serialize)]
pub struct Meta {
    /// Tab ID if the command operated on a tab.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    /// Wall-clock time in milliseconds.
    pub elapsed_ms: u64,
}

impl CliResponse {
    /// Create a success response with data.
    pub fn success(data: Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            error_code: None,
            meta: None,
        }
    }

    /// Create a success response with data and metadata.
    pub fn success_with_meta(data: Value, tab_id: Option<String>, elapsed_ms: u64) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            error_code: None,
            meta: Some(Meta { tab_id, elapsed_ms }),
        }
    }

    /// Create an error response.
    pub fn error(error: impl Into<String>, error_code: impl AsRef<str>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(error.into()),
            error_code: Some(error_code.as_ref().to_string()),
            meta: None,
        }
    }

    /// Create an error response with metadata.
    #[allow(dead_code)]
    pub fn error_with_meta(
        error: impl Into<String>,
        error_code: impl AsRef<str>,
        tab_id: Option<String>,
        elapsed_ms: u64,
    ) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(error.into()),
            error_code: Some(error_code.as_ref().to_string()),
            meta: Some(Meta { tab_id, elapsed_ms }),
        }
    }

    /// Attach metadata to an existing response.
    pub fn with_meta(mut self, tab_id: Option<String>, elapsed_ms: u64) -> Self {
        self.meta = Some(Meta { tab_id, elapsed_ms });
        self
    }

    /// Create CliResponse from an InputError.
    pub fn from_validation(e: crate::validate::InputError) -> Self {
        CliResponse::error(e.to_string(), e.error_code())
    }

/// Determine the exit code from the error_code field.
    pub fn exit_code(&self) -> i32 {
        if self.ok {
            return exit_code::OK;
        }
        match self.error_code.as_deref() {
            Some("INVALID_URL")
            | Some("INVALID_SELECTOR")
            | Some("INPUT_VALIDATION")
            | Some("PATH_TRAVERSAL")
            | Some("SSRF_BLOCKED") => exit_code::INPUT,
            Some("TIMEOUT") => exit_code::TIMEOUT,
            Some("NETWORK_ERROR") | Some("HTTP_ERROR") => exit_code::NETWORK,
            _ => exit_code::RUNTIME,
        }
    }

    /// Serialize and print as JSON to stdout. Always includes meta.
    pub fn print_json(&self) -> i32 {
        let code = self.exit_code();
        let mut resp = serde_json::to_value(self).unwrap_or_else(|e| {
            serde_json::json!({"ok":false,"error":format!("serialization: {e}"),"error_code":"INTERNAL"})
        });
        // Ensure meta is always present
        if let Some(obj) = resp.as_object_mut() {
            match obj.get_mut("meta") {
                Some(meta) if !meta.is_null() => {}
                _ => {
                    obj.insert("meta".into(), serde_json::json!({"elapsed_ms": 0}));
                }
            }
        }
        println!("{}", serde_json::to_string(&resp).unwrap_or_default());
        code
    }

    /// Alias for print_json (backwards compat).
    #[allow(dead_code)]
    pub fn print(&self) -> i32 {
        self.print_json()
    }

    /// Print and exit the process.
    #[allow(dead_code)]
    pub fn print_and_exit(&self) -> ! {
        std::process::exit(self.print_json());
    }
}

// ---------------------------------------------------------------------------
// Output format detection
// ---------------------------------------------------------------------------

/// Whether stdout is a TTY (terminal).
pub fn is_stdout_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Whether JSON output should be used.
///
/// JSON is used when:
/// 1. `--json` flag is explicitly set, OR
/// 2. stdout is not a TTY (piped/redirected)
pub fn should_output_json(explicit_json: bool) -> bool {
    explicit_json || !is_stdout_tty()
}

/// Whether JSON output should be used (alias).
#[allow(dead_code)]
pub fn json_mode(explicit_json: bool) -> bool {
    should_output_json(explicit_json)
}

// ---------------------------------------------------------------------------
// Truncation
// ---------------------------------------------------------------------------

/// Truncate string fields within a JSON object to fit within `max_bytes`.
///
/// Adds `truncated`, `total_bytes`, `returned_bytes` to the object.
/// Only truncates string-valued fields. Non-string fields are left intact
/// but counted toward the budget.
pub fn truncate_fields(data: &mut Value, max_bytes: u64) {
    if max_bytes == 0 {
        return;
    }

    let obj = match data.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // Measure total string content
    let mut total: usize = 0;
    let mut string_keys: Vec<String> = Vec::new();
    for (key, val) in obj.iter() {
        if let Some(s) = val.as_str() {
            total += s.len();
            string_keys.push(key.clone());
        }
    }

    if total <= max_bytes as usize {
        return;
    }

    // Truncate: allocate budget proportionally, prioritize earlier fields
    let non_string_size: usize = obj
        .iter()
        .filter(|(_, v)| !v.is_string())
        .map(|(_, v)| v.to_string().len())
        .sum();

    let string_budget = max_bytes as usize - non_string_size.min(max_bytes as usize);
    let mut used = 0usize;

    for key in &string_keys {
        let val = obj.get_mut(key.as_str()).unwrap();
        if let Some(s) = val.as_str() {
            let remaining = string_budget.saturating_sub(used);
            if s.len() <= remaining {
                used += s.len();
            } else {
                // Truncate at char boundary
                let mut cut = remaining;
                while !s.is_char_boundary(cut) && cut > 0 {
                    cut -= 1;
                }
                *val = Value::String(s[..cut].to_string());
                used += cut;
            }
        }
    }

    obj.insert("truncated".into(), Value::Bool(true));
    obj.insert("total_bytes".into(), serde_json::json!(total));
    obj.insert("returned_bytes".into(), serde_json::json!(max_bytes));
}

// ---------------------------------------------------------------------------
// Field filtering
// ---------------------------------------------------------------------------

/// Filter a JSON object to only include specified field names.
pub fn filter_fields(data: &mut Value, fields: &[&str]) {
    if fields.is_empty() {
        return;
    }

    let obj = match data.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    let allowed: std::collections::HashSet<&str> = fields.iter().copied().collect();
    let keys_to_remove: Vec<String> = obj
        .keys()
        .filter(|k| !allowed.contains(k.as_str()))
        .cloned()
        .collect();

    for key in keys_to_remove {
        obj.remove(&key);
    }
}

/// Parse a comma-separated fields string into a vec of field names.
pub fn parse_fields(fields: &str) -> Vec<&str> {
    fields.split(',').map(|f| f.trim()).filter(|f| !f.is_empty()).collect()
}

// ---------------------------------------------------------------------------
// Page summary
// ---------------------------------------------------------------------------

/// Build a page summary from a loaded page.
pub fn build_summary(page: &oxibrowser_core::page::Page) -> Value {
    let doc = page.root_frame().document();
    let headings: Vec<String> = {
        let h_tags = ["h1", "h2", "h3", "h4", "h5", "h6"];
        let mut result = Vec::new();
        for tag in h_tags {
            let ids = doc.query_selector_all(tag);
            for id in ids {
                if let Some(text) = doc.text_content(id) {
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        result.push(trimmed);
                    }
                }
            }
        }
        result
    };

    let links_count = doc.query_selector_all("a[href]").len();
    let forms_count = doc.query_selector_all("form").len();
    let images_count = doc.query_selector_all("img").len();
    let text_length = doc.query_text("body").map(|t| t.len()).unwrap_or(0);

    serde_json::json!({
        "url": page.url().to_string(),
        "title": page.title().unwrap_or(""),
        "status": page.status(),
        "content_type": page.content_type(),
        "headings": headings,
        "links_count": links_count,
        "forms_count": forms_count,
        "images_count": images_count,
        "text_length": text_length,
    })
}

// ---------------------------------------------------------------------------
// Core error → error_code mapping
// ---------------------------------------------------------------------------

/// Map a core error to a machine-readable error code string.
pub fn core_error_code(error: &oxibrowser_core::error::CoreError) -> &'static str {
    let msg = format!("{error}");
    if msg.contains("timeout") || msg.contains("timed out") {
        "TIMEOUT"
    } else if msg.contains("no element") || msg.contains("not found") {
        "DOM_NOT_FOUND"
    } else if msg.contains("JsError") || msg.contains("JS") {
        "JS_ERROR"
    } else if msg.contains("network") || msg.contains("dns") || msg.contains("connect") {
        "NETWORK_ERROR"
    } else if msg.contains("HTTP") || msg.contains("status") {
        "HTTP_ERROR"
    } else if msg.contains("page not loaded") {
        "PAGE_NOT_LOADED"
    } else if msg.contains("closed") {
        "TAB_CLOSED"
    } else {
        "RUNTIME_ERROR"
    }
}

/// Map a core error to an exit code.
#[allow(dead_code)]
pub fn core_exit_code(error: &oxibrowser_core::error::CoreError) -> i32 {
    match core_error_code(error) {
        "TIMEOUT" => exit_code::TIMEOUT,
        "NETWORK_ERROR" | "HTTP_ERROR" => exit_code::NETWORK,
        _ => exit_code::RUNTIME,
    }
}

// ---------------------------------------------------------------------------
// Print helpers for non-JSON mode
// ---------------------------------------------------------------------------

/// Print a BrowseResult in human-readable format.
pub fn print_human_result(
    page: &oxibrowser_core::page::Page,
    format: &str,
    show_headers: bool,
) {
    if show_headers {
        eprintln!("HTTP {}", page.status());
        eprintln!("Content-Type: {}", page.content_type());
    }

    match format {
        "markdown" | "md" => print!("{}", page.to_markdown()),
        "text" => {
            let body = page
                .root_frame()
                .document()
                .query_text("body")
                .unwrap_or_default();
            print!("{body}");
        }
        _ => print!("{}", page.content()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_success_serializes() {
        let r = CliResponse::success(serde_json::json!({"url": "https://example.com"}));
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""ok":true"#));
        assert!(json.contains(r#""url":"https://example.com""#));
        assert!(!json.contains("error"));
    }

    #[test]
    fn test_response_error_serializes() {
        let r = CliResponse::error("not found", "DOM_NOT_FOUND");
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""ok":false"#));
        assert!(json.contains("not found"));
        assert!(json.contains("DOM_NOT_FOUND"));
        assert!(!json.contains("data"));
    }

    #[test]
    fn test_exit_code_mapping() {
        assert_eq!(
            CliResponse::error("x", "DOM_NOT_FOUND").exit_code(),
            exit_code::RUNTIME
        );
        assert_eq!(
            CliResponse::error("x", "INVALID_URL").exit_code(),
            exit_code::INPUT
        );
        assert_eq!(
            CliResponse::error("x", "TIMEOUT").exit_code(),
            exit_code::TIMEOUT
        );
        assert_eq!(
            CliResponse::error("x", "NETWORK_ERROR").exit_code(),
            exit_code::NETWORK
        );
        assert_eq!(
            CliResponse::success(serde_json::json!({})).exit_code(),
            exit_code::OK
        );
    }

    #[test]
    fn test_truncate_fields_no_truncation_needed() {
        let mut data = serde_json::json!({"text": "hello"});
        truncate_fields(&mut data, 100);
        // No truncation needed → no `truncated` key added
        assert!(data.get("truncated").is_none());
    }

    #[test]
    fn test_truncate_fields_truncates() {
        let mut data = serde_json::json!({"a": "short", "b": "a very long string that exceeds the limit"});
        truncate_fields(&mut data, 10);
        assert!(data.get("truncated").unwrap().as_bool().unwrap());
        assert!(data.get("total_bytes").unwrap().as_u64().unwrap() > 10);
        assert_eq!(data.get("returned_bytes").unwrap().as_u64().unwrap(), 10);
    }

    #[test]
    fn test_filter_fields() {
        let mut data = serde_json::json!({"url": "https://example.com", "title": "Example", "html": "<html>"});
        filter_fields(&mut data, &["url", "title"]);
        assert!(data.get("url").is_some());
        assert!(data.get("title").is_some());
        assert!(data.get("html").is_none());
    }

    #[test]
    fn test_parse_fields() {
        let fields = parse_fields("url, title, status");
        assert_eq!(fields, vec!["url", "title", "status"]);
    }
}
