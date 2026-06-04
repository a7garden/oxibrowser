//! Browser lifecycle events surfaced to observers.
//!
//! This is the public observability surface of oxibrowser-core.
//! Surfaced through `Browser::subscribe_events()` for observers
//! (oxi-agent, CDP, MCP) to forward to upstream consumers.
//!
//! Scope is intentionally narrow: only state transitions the user
//! would care about for "what is my agent doing right now?" visibility.
//! Low-level network details (DNS, TLS, sub-resources) are NOT here.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Lifecycle events emitted by the browser.
///
/// Keep this enum small and `#[non_exhaustive]` — adding variants
/// is backwards-compatible; reordering/renaming is not.
///
/// These events are emitted on a `tokio::sync::broadcast` channel inside
/// `Browser`. Observers (e.g. oxi-agent) subscribe and forward the
/// `short_label()` to the agent loop's `ToolExecutionUpdate` callback.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BrowserEvent {
    /// `Tab::goto` has begun. Emitted before any network I/O.
    NavigationStarted {
        /// The URL the caller asked for (pre-redirect).
        url: String,
    },

    /// `Tab::wait_for` is polling for a CSS selector.
    WaitingForSelector {
        /// The CSS selector being awaited.
        selector: String,
        /// Maximum time we'll wait before giving up.
        timeout_ms: u64,
    },

    /// The page has finished loading and JS has executed.
    ///
    /// This is the single "page is done" signal. It includes
    /// enough information to render a meaningful one-line summary.
    DocumentReady {
        /// Final URL after any redirects.
        final_url: String,
        /// Page `<title>`.
        title: String,
        /// HTTP status code of the main response.
        status: u16,
        /// Total bytes received for the main document.
        total_bytes: u64,
        /// Number of `<script>` blocks the page references.
        js_script_count: usize,
        /// Wall-clock duration of the whole `goto` call.
        total_duration: Duration,
    },

    /// A screenshot has been rendered and captured.
    ScreenshotCaptured {
        /// Size of the PNG payload, in bytes.
        bytes: usize,
        /// Viewport width the screenshot was rendered at.
        viewport_width: u32,
        /// Wall-clock duration of the render.
        duration: Duration,
    },
}

impl BrowserEvent {
    /// Short human-readable label suitable for a UI card.
    ///
    /// This is the single source of truth for the user-facing text.
    /// The UI layer does not format anything; it just renders the
    /// returned string as the `progress` line of the tool card.
    pub fn short_label(&self) -> String {
        match self {
            Self::NavigationStarted { url } => format!("Opening {url}…"),

            Self::WaitingForSelector {
                selector,
                timeout_ms,
            } => {
                let secs = timeout_ms / 1000;
                format!("Waiting for `{selector}` (up to {secs}s)…")
            }

            Self::DocumentReady {
                title,
                status,
                total_bytes,
                js_script_count,
                total_duration,
                ..
            } => {
                let ms = total_duration.as_millis();
                format!(
                    "Loaded \"{title}\" — {status} · {} · {js_script_count} scripts · {ms} ms",
                    human_bytes(*total_bytes),
                )
            }

            Self::ScreenshotCaptured {
                bytes,
                viewport_width,
                duration,
            } => {
                let ms = duration.as_millis();
                format!(
                    "Screenshot ready — {} · {viewport_width} px · {ms} ms",
                    human_bytes(*bytes as u64),
                )
            }
        }
    }
}

fn human_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_started_label() {
        let e = BrowserEvent::NavigationStarted {
            url: "https://example.com".into(),
        };
        assert_eq!(e.short_label(), "Opening https://example.com…");
    }

    #[test]
    fn waiting_for_selector_label() {
        let e = BrowserEvent::WaitingForSelector {
            selector: ".content".into(),
            timeout_ms: 30_000,
        };
        assert_eq!(e.short_label(), "Waiting for `.content` (up to 30s)…");
    }

    #[test]
    fn document_ready_label() {
        let e = BrowserEvent::DocumentReady {
            final_url: "https://example.com".into(),
            title: "Example".into(),
            status: 200,
            total_bytes: 1256,
            js_script_count: 2,
            total_duration: Duration::from_millis(245),
        };
        let s = e.short_label();
        assert!(s.contains("Example"), "missing title: {s}");
        assert!(s.contains("200"), "missing status: {s}");
        assert!(s.contains("1.2 KB"), "missing bytes: {s}");
        assert!(s.contains("2 scripts"), "missing script count: {s}");
        assert!(s.contains("245 ms"), "missing duration: {s}");
    }

    #[test]
    fn screenshot_captured_label() {
        let e = BrowserEvent::ScreenshotCaptured {
            bytes: 8192,
            viewport_width: 800,
            duration: Duration::from_millis(50),
        };
        let s = e.short_label();
        assert!(s.contains("8.0 KB"), "missing bytes: {s}");
        assert!(s.contains("800 px"), "missing width: {s}");
        assert!(s.contains("50 ms"), "missing duration: {s}");
    }

    #[test]
    fn event_serializes_with_kind_tag() {
        let e = BrowserEvent::NavigationStarted {
            url: "https://x".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            json.contains(r#""kind":"navigation_started""#),
            "expected snake_case kind tag, got: {json}"
        );
    }

    #[test]
    fn all_events_have_labels() {
        // Sanity: every variant has a label match arm.
        let events = vec![
            BrowserEvent::NavigationStarted {
                url: "https://x".into(),
            },
            BrowserEvent::WaitingForSelector {
                selector: ".a".into(),
                timeout_ms: 1000,
            },
            BrowserEvent::DocumentReady {
                final_url: "https://x".into(),
                title: "t".into(),
                status: 200,
                total_bytes: 0,
                js_script_count: 0,
                total_duration: Duration::from_millis(0),
            },
            BrowserEvent::ScreenshotCaptured {
                bytes: 0,
                viewport_width: 0,
                duration: Duration::from_millis(0),
            },
        ];
        for e in events {
            let s = e.short_label();
            assert!(!s.is_empty(), "label should not be empty for {e:?}");
        }
    }
}
