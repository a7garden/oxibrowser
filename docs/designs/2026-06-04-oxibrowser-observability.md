# v0.12: OxiBrowser Observability for Agent UIs

> **Status:** Design (final)
> **Author:** oxibrowser team
> **Scope:** oxibrowser-core → oxi-agent → oxi-sdk → oxios-kernel → oxios-web
> **Estimated effort:** P0+P1 ≈ 500 LoC Rust + 50 LoC TS, ~1 week

---

## 1. Goal

Make OxiBrowser emit a small set of **lifecycle events** (open, load, wait, screenshot) that bubble all the way up to the Oxios web UI, so users can see *what their agent is doing in the browser* — not for flashy rendering, but for **observability transparency** ("the agent is opening X, it's waiting for Y, the page is ready").

### Design principles

1. **Few events, high signal.** 4 event variants total. Each one is a meaningful state transition, not an internal step.
2. **No low-level network noise.** DNS / TLS / connection / sub-resource fetches are not surfaced — they are implementation details the user does not need to see.
3. **No fancy render in oxibrowser.** The browser emits structured data. The UI decides how to display.
4. **The UI already has a `tool_call` card.** We extend it with a `progress` text line and a spinner. Nothing else changes.

### The 4 events

| Event | When | Example UI text |
|-------|------|-----------------|
| `NavigationStarted { url }` | At the beginning of `Tab::goto` | "Opening https://example.com…" |
| `WaitingForSelector { selector, timeout_ms }` | At the beginning of `Tab::wait_for` | "Waiting for '.content' (up to 30s)…" |
| `DocumentReady { final_url, title, status, total_bytes, js_script_count, total_duration }` | After the page is fully loaded and JS executed | "Loaded \"Example Domain\" — 12 KB · 200 · 4 scripts · 245 ms" |
| `ScreenshotCaptured { bytes, viewport_width, duration }` | After a screenshot is rendered | "Screenshot ready — 8 KB · 800 px · 50 ms" |

The full event chain reaches the UI as:

```
oxibrowser-core  →  oxi-agent  →  oxios-kernel  →  oxios-web
  BrowserEvent       ProgressCb      KernelEvent      WS chunk
                     (String)         (Progress)       (tool_progress)
```

### What this plan is **not**

- **No per-resource events** (no per-script, per-CSS, per-image events). Those are noise; transparency does not need them.
- **No CDP extension** in this version. `OXI.observe` is a *P2 nice-to-have*, not part of MVP.
- **No UI rewrite.** The existing `ActivityCard` + `ActivityTimeline` already render `tool_call`. We add a `progress` field and a spinner.
- **No progress bar / percentage.** We surface *what's happening* (text), not *how far along* (number). The total_duration in the final event is the only numeric.

---

## 2. Key discovery — most plumbing is already in place

I traced the call chain before writing this plan. Several pieces **already exist** and need only to be *connected*, not built:

| Already exists | Location | Status |
|----------------|----------|--------|
| `AgentTool::on_progress(callback)` trait method | `oxi-agent/src/tools.rs:280` | ✅ default no-op; tools override |
| `ProgressCallback = Arc<dyn Fn(String) + Send + Sync>` | `oxi-agent/src/tools.rs:138` | ✅ |
| `AgentEvent::ToolExecutionUpdate { tool_call_id, tool_name, partial_result }` | `oxi-agent/src/events.rs:79` | ✅ |
| Agent loop wires the callback before `execute()` | `oxi-agent/src/agent_loop/tool_exec.rs:441` | ✅ |
| `oxi-sdk` re-exports `AgentEvent` | `oxi-sdk/src/lib.rs:175` | ✅ |
| `KernelEvent` has `ToolExecutionStarted` / `Finished` | `oxios-kernel/src/event_bus.rs:165` | ⚠️ **no `Progress` variant** |
| `agent_runtime.rs` converts `AgentEvent` → `KernelEvent` for SSE/WS | `oxios-kernel/src/agent_runtime.rs:680-750` | ⚠️ **no `ToolExecutionUpdate` arm** |
| `oxios-web` WS chunk `kernel_event_to_ws_chunk` | `oxios-web/src/routes/chat.rs:691` | ⚠️ **no `tool_progress` arm** |
| Frontend `chunkToActivity` / `ActivityCard` | `oxios-web/web/src/stores/chat.ts:89`, `components/chat/activity-card.tsx` | ⚠️ **no `tool_progress` case** |
| `oxibrowser-core::Browser` has `tokio::sync::broadcast` for shutdown | `oxibrowser-core/src/browser.rs:23` | ✅ pattern reusable |
| `oxibrowser-core` has no event subscription API | — | ❌ needs to be added |

**The remaining work is one new event stream in oxibrowser-core, plus three thin forwarding layers in oxi-agent, oxios-kernel, and oxios-web.**

---

## 3. Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│ oxibrowser-core                                                     │
│                                                                     │
│  Browser::events: broadcast::Sender<BrowserEvent>   ← NEW          │
│  Session / Tab / Page emit during goto, click, screenshot…         │
│                                                                     │
└──────────────────────────┬──────────────────────────────────────────┘
                           │ subscribe_events()
                           ▼
┌─────────────────────────────────────────────────────────────────────┐
│ oxi-agent (oxi_agent::tools::browse::oxibrowser_backend)            │
│                                                                     │
│  OxiBrowserEngine:                                                  │
│    - on construction: subscribe to Browser::events                  │
│    - on each event: invoke stored ProgressCallback (if any)        │
│                                                                     │
│  OxiTab:                                                            │
│    - delegates progress to engine's stored callback                 │
│                                                                     │
│  BrowseTool (oxi-agent/src/tools/browse/browse_tool.rs):            │
│    - override on_progress() to store the callback                   │
│    - in execute(): forward ProgressCallback to OxiTab via engine    │
│                                                                     │
└──────────────────────────┬──────────────────────────────────────────┘
                           │ AgentEvent::ToolExecutionUpdate
                           ▼
┌─────────────────────────────────────────────────────────────────────┐
│ oxios-kernel (agent_runtime.rs)                                    │
│                                                                     │
│  In the streaming event callback:                                  │
│    AgentEvent::ToolExecutionUpdate { .. } =>                       │
│      KernelEvent::ToolExecutionProgress { session_id, .. }         │
│                                                                     │
│  Published on EventBus → /api/events SSE                           │
│                          → /api/chat/stream WS                     │
│                                                                     │
└──────────────────────────┬──────────────────────────────────────────┘
                           │ WS chunk: { type: "tool_progress", … }
                           ▼
┌─────────────────────────────────────────────────────────────────────┐
│ oxios-web (chat.ts + activity-card.tsx)                            │
│                                                                     │
│  chunkToActivity('tool_progress') → ChatActivity                   │
│    { type: 'tool_call', isRunning: true, progress: '…' }            │
│  ActivityCard: show Loader2 spinner while progress field is set    │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 4. Detailed implementation

### 4.1 oxibrowser-core — `BrowserEvent` + emitter

**New file: `crates/oxibrowser-core/src/event.rs`**

```rust
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
        /// Number of `<script>` blocks executed.
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

            Self::WaitingForSelector { selector, timeout_ms } => {
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
        assert!(s.contains("Example"));
        assert!(s.contains("200"));
        assert!(s.contains("1.2 KB"));
        assert!(s.contains("2 scripts"));
        assert!(s.contains("245 ms"));
    }

    #[test]
    fn event_serializes_with_kind_tag() {
        let e = BrowserEvent::NavigationStarted {
            url: "https://x".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""kind":"navigation_started""#));
    }
}
```

**Wire it into `Browser`:**

`crates/oxibrowser-core/src/browser.rs` — add a broadcast channel next to the existing `shutdown_tx`:

```rust
use tokio::sync::broadcast;

pub struct Browser {
    // ... existing fields ...
    shutdown_tx: broadcast::Sender<()>,
    /// Live browser events. 32-slot buffer is plenty — we only have
    /// 4 event variants per page load, and the agent drops oldest on overflow.
    event_tx: broadcast::Sender<BrowserEvent>,
}

impl Browser {
    pub async fn new(config: BrowserConfig) -> Result<Self> {
        // ... existing setup ...
        let (shutdown_tx, _) = broadcast::channel::<()>(1);
        // 32 slots = generous headroom; we emit ≤4 events per page load
        // (NavigationStarted, optional WaitingForSelector, DocumentReady,
        //  optional ScreenshotCaptured). The agent drops oldest on overflow.
        let (event_tx, _) = broadcast::channel::<BrowserEvent>(32);
        Ok(Self {
            // ...
            shutdown_tx,
            event_tx,
        })
    }

    /// Subscribe to live browser events.
    pub fn subscribe_events(&self) -> broadcast::Receiver<BrowserEvent> {
        self.event_tx.subscribe()
    }

    /// Internal: emit a browser event. Never blocks.
    /// On overflow, the oldest event is dropped (broadcast semantics).
    pub(crate) fn emit_event(&self, event: BrowserEvent) {
        let _ = self.event_tx.send(event);
    }
}
```

**Emit during `Tab::goto`, `Tab::wait_for`, and `Tab::screenshot`:**

`crates/oxibrowser-core/src/tab.rs` — insert `emit_event` calls at the boundaries. The existing functions stay intact; the emits are pure additions:

```rust
pub async fn goto(&self, url: &str) -> Result<BrowseResult> {
    let started = std::time::Instant::now();
    self.browser.emit_event(BrowserEvent::NavigationStarted {
        url: url.to_string(),
    });

    // ... existing URL fetch, HTML parse, JS exec logic, unchanged ...

    // After the page is fully loaded and JS has executed, just before
    // returning the BrowseResult, emit the single "done" event:
    self.browser.emit_event(BrowserEvent::DocumentReady {
        final_url: result.url.clone(),
        title: result.title.clone(),
        status: result.status,
        total_bytes: result.bytes,                  // add field if missing
        js_script_count: script_count,              // already computed locally
        total_duration: started.elapsed(),
    });

    Ok(result)
}

pub async fn wait_for(&self, selector: &str, timeout_ms: u64) -> Result<()> {
    self.browser.emit_event(BrowserEvent::WaitingForSelector {
        selector: selector.to_string(),
        timeout_ms,
    });

    // ... existing polling logic, unchanged ...
}

pub async fn screenshot(&self, width: u32) -> Result<Vec<u8>> {
    let started = std::time::Instant::now();

    // ... existing render code, unchanged ...

    self.browser.emit_event(BrowserEvent::ScreenshotCaptured {
        bytes: png.len(),
        viewport_width: width,
        duration: started.elapsed(),
    });
    Ok(png)
}
```

> **Note on the existing `goto` body:** if `result.bytes` and `script_count` are not already extracted locally, this PR adds the small extra work of computing them. `total_bytes` can come from the response `Content-Length` header (or accumulated body length if chunked). `js_script_count` is the count of `<script>` blocks that the JS runtime actually executed — most pages have a small finite number; fall back to `0` if the count is not tracked.

`crates/oxibrowser-core/src/lib.rs`:
```rust
pub mod event;
pub use event::BrowserEvent;
```

**Bump version:** `crates/oxibrowser-core/Cargo.toml` → `0.11.0` → `0.12.0` (additive, no breakage).

---

### 4.2 oxi-agent — wire `BrowserEvent` to `ProgressCallback`

**Modify `BrowserEngine` trait:** `oxi-agent/src/tools/browse/engine.rs`

```rust
use std::sync::Mutex;

pub struct ProgressForwarder {
    callback: Mutex<Option<crate::tools::ProgressCallback>>,
}

impl ProgressForwarder {
    pub fn new() -> Self { Self { callback: Mutex::new(None) } }
    pub fn set(&self, cb: crate::tools::ProgressCallback) {
        *self.callback.lock().unwrap() = Some(cb);
    }
    pub fn invoke(&self, msg: String) {
        if let Some(cb) = self.callback.lock().unwrap().as_ref() {
            cb(msg);
        }
    }
}

#[async_trait]
pub trait BrowserEngine: Send + Sync {
    // ... existing methods ...

    /// Access the engine's progress forwarder. Tools use this to receive
    /// the on_progress callback from the agent loop.
    fn progress_forwarder(&self) -> Arc<ProgressForwarder>;

    /// Optional: subscribe to raw browser events. None by default.
    fn subscribe_events(&self) -> Option<tokio::sync::broadcast::Receiver<oxibrowser_core::BrowserEvent>> {
        None
    }
}
```

**Default impl in `MockTab` / mock engines** (if any): just return a new `Arc<ProgressForwarder>`.

**`OxiBrowserEngine` impl:** `oxi-agent/src/tools/browse/oxibrowser_backend.rs`

```rust
pub struct OxiBrowserEngine {
    browser: oxibrowser_core::Browser,
    config: BrowseConfig,
    progress: Arc<ProgressForwarder>,
    /// Background task draining BrowserEvent → ProgressForwarder.
    _event_task: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl OxiBrowserEngine {
    pub async fn with_config(config: BrowseConfig) -> Result<Self, BrowserError> {
        // ... existing setup ...
        let browser = oxibrowser_core::Browser::new(browser_config).await?;
        let progress = Arc::new(ProgressForwarder::new());
        let mut events_rx = browser.subscribe_events();
        let progress_clone = Arc::clone(&progress);
        let task = tokio::spawn(async move {
            while let Ok(event) = events_rx.recv().await {
                progress_clone.invoke(event.short_label());
            }
        });
        Ok(Self {
            browser, config, progress,
            _event_task: parking_lot::Mutex::new(Some(task)),
        })
    }
}

impl BrowserEngine for OxiBrowserEngine {
    fn progress_forwarder(&self) -> Arc<ProgressForwarder> {
        Arc::clone(&self.progress)
    }
    fn subscribe_events(&self) -> Option<tokio::sync::broadcast::Receiver<oxibrowser_core::BrowserEvent>> {
        Some(self.browser.subscribe_events())
    }
}
```

**`BrowseTool` override `on_progress`:** `oxi-agent/src/tools/browse/browse_tool.rs`

```rust
#[async_trait]
impl AgentTool for BrowseTool {
    // ... existing methods ...

    fn on_progress(&self, callback: crate::tools::ProgressCallback) {
        self.engine.progress_forwarder().set(callback);
    }

    async fn execute(...) -> Result<AgentToolResult, ToolError> {
        // ... existing body unchanged ...
        // The callback fires whenever OxiBrowserEngine emits a BrowserEvent.
    }
}
```

**Bump version:** `oxi-agent/Cargo.toml` → `0.26.2` → `0.27.0`. `oxi-sdk` → `0.27.0`.

**`oxi-sdk` re-exports:** `oxi-sdk/src/lib.rs` — add `BrowserEvent` and `ResourceKind` to the re-export list (guarded by `native-browser`).

---

### 4.3 oxios-kernel — convert `ToolExecutionUpdate` to `KernelEvent`

**`KernelEvent` enum:** `oxios-kernel/src/event_bus.rs` — add a new variant:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum KernelEvent {
    // ... existing variants ...

    /// A tool execution is making progress (real-time, RFC-015+).
    ToolExecutionProgress {
        session_id: String,
        tool_call_id: String,
        tool_name: String,
        /// Short human-readable progress message from the tool.
        progress: String,
    },
}
```

**`audit_action` mapping:** add an arm for the new variant (use `AuditAction::Other` with `detail: "tool_progress:..."`).

**`agent_runtime.rs` event callback:** at line ~680, add a new arm *between* `ToolExecutionStart` and `ToolExecutionEnd`:

```rust
AgentEvent::ToolExecutionUpdate {
    tool_call_id,
    tool_name,
    partial_result,
} => {
    if let Some(ref sid) = transparency_session {
        let _ = kernel_handle_for_cb.infra.publish(
            KernelEvent::ToolExecutionProgress {
                session_id: sid.clone(),
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                progress: partial_result,
            },
        );
    }
}
```

**`oxios-web` SSE sanitizer:** `oxios-web/src/routes/events.rs` — add a match arm:

```rust
KernelEvent::ToolExecutionProgress {
    session_id, tool_call_id, tool_name, progress
} => serde_json::json!({
    "type": "tool_progress",
    "session_id": session_id,
    "tool_call_id": tool_call_id,
    "tool_name": tool_name,
    "progress": progress,
}),
```

**`oxios-web` WS chunk converter:** `oxios-web/src/routes/chat.rs:691` — add a match arm and extend the session-id filter list:

```rust
let event_session_id: Option<&str> = match event {
    // ... existing arms ...
    KernelEvent::ToolExecutionProgress { session_id, .. } => Some(session_id),
    _ => None,
};

match event {
    // ... existing arms ...

    KernelEvent::ToolExecutionProgress {
        tool_call_id, tool_name, progress, ..
    } => Some(serde_json::json!({
        "type": "tool_progress",
        "tool_call_id": tool_call_id,
        "tool_name": tool_name,
        "progress": progress,
    })),
    _ => None,
}
```

**Add a unit test** in the existing `rfc015_tests` block to keep the wire-contract guarantee.

**Bump versions:** `oxios-kernel` → 1.0.3, `oxios-web` → 1.0.3 (minor bump because `#[non_exhaustive]` enum changed).

---

### 4.4 oxios-web frontend — `tool_progress` chunk

**Type:** `oxios-web/web/src/types/index.ts:247` — add to `StreamChunk` union:

```typescript
| { type: 'tool_progress'; tool_call_id: string; tool_name: string; progress: string }
```

**ChatActivity:** add an optional `progress` field to the `tool_call` variant:

```typescript
type ChatActivity =
  | { type: 'tool_call'; ...; progress?: string; isRunning?: boolean }
  | ...
```

**`chunkToActivity`:** `oxios-web/web/src/stores/chat.ts:89` — handle the new case **and** the existing `tool_start` should set `isRunning: true`:

```typescript
case 'tool_start':
  return {
    id: baseId(chunk.tool_call_id),
    type: 'tool_call',
    timestamp: ts,
    toolName: chunk.tool_name,
    toolCallId: chunk.tool_call_id,
    toolArgs: chunk.tool_args,
    isRunning: true,
  }
case 'tool_progress':
  return {
    id: baseId(chunk.tool_call_id),
    type: 'tool_call',
    timestamp: ts,
    toolName: chunk.tool_name,
    toolCallId: chunk.tool_call_id,
    progress: chunk.progress,
    isRunning: true,
  }
case 'tool_end':
  return {
    id: baseId(chunk.tool_call_id),
    type: 'tool_call',
    timestamp: ts,
    toolName: chunk.tool_name,
    toolCallId: chunk.tool_call_id,
    outputSummary: chunk.output_summary,
    durationMs: chunk.duration_ms,
    isError: chunk.is_error,
    isRunning: false,  // explicit
  }
```

**`ActivityCard`:** `oxios-web/web/src/components/chat/activity-card.tsx` — show a spinner when `isRunning`, and surface `progress` text as a thin status line:

```tsx
import { Loader2 } from 'lucide-react'

// Inside ActivityCard, replace the header:
<span className="font-medium truncate">{label}</span>
{activity.isRunning && <Loader2 className="h-3 w-3 animate-spin text-muted-foreground" />}
{activity.progress && (
  <span className="text-[10px] text-muted-foreground truncate">
    {activity.progress}
  </span>
)}
{badge}
```

**Add tests** in `__tests__/stores.test.ts` for the new chunk type.

---

## 5. P2 — Nice-to-haves (not part of MVP)

These are explicitly **out of scope** for v0.12 but worth noting so they don't get forgotten:

| P2 item | Where | Effort |
|---------|-------|--------|
| Per-resource events (CSS, image, script, font) | `oxibrowser-core/src/network/` | 100 LoC |
| CDP `OXI.observe` domain | `oxibrowser-cdp/src/domains/oxi.rs` (new file) | 200 LoC |
| MCP `notifications/progress` | `oxibrowser/src/main.rs` new `mcp-server` subcommand | 150 LoC |
| Structured progress (`ToolProgress::Percentage` etc.) | `oxi-agent/src/tools/browse/browse_tool.rs` | 80 LoC |
| Persist progress fragments to session log | `oxios-kernel/src/state_store.rs` | 60 LoC |

---

## 6. Testing strategy

### Unit tests (run with `cargo test --workspace`)

| Test | Location | Verifies |
|------|----------|----------|
| `navigation_started_label` | `oxibrowser-core/src/event.rs` | label format |
| `document_ready_label` | `oxibrowser-core/src/event.rs` | label format (status, bytes, scripts, ms) |
| `event_serializes_with_kind_tag` | `oxibrowser-core/src/event.rs` | wire format |
| `emit_event_does_not_block_on_no_subscribers` | `oxibrowser-core/src/browser.rs` | overflow safety |
| `OxiBrowserEngine_forwards_event_to_progress_callback` | `oxi-agent/src/tools/browse/oxibrowser_backend.rs` | end-to-end event flow |
| `BrowseTool_on_progress_stores_callback` | `oxi-agent/src/tools/browse/browse_tool.rs` | trait impl |
| `kernel_event_tool_progress_serializes` | `oxios-kernel/src/event_bus.rs` | enum invariant |
| `tool_progress_emits_tool_progress_chunk` | `oxios-web/src/routes/chat.rs` (extend `rfc015_tests`) | wire contract |
| `chunkToActivity_tool_progress` | `oxios-web/web/src/__tests__/stores.test.ts` | frontend type |

### Integration tests (with `#[tokio::test]`)

```rust
#[tokio::test]
async fn goto_emits_navigation_and_ready() {
    let engine = OxiBrowserEngine::new().await.unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut events = engine.subscribe_events().unwrap();
    tokio::spawn(async move {
        while let Ok(e) = events.recv().await {
            let _ = tx.send(e);
        }
    });

    let tab = engine.new_tab().await.unwrap();
    tab.goto("https://example.com").await.unwrap();

    // Expect at least NavigationStarted and DocumentReady
    let mut got_started = false;
    let mut got_ready = false;
    for _ in 0..10 {
        if let Some(e) = rx.recv().await {
            match e {
                BrowserEvent::NavigationStarted { .. } => got_started = true,
                BrowserEvent::DocumentReady { .. } => got_ready = true,
                _ => {}
            }
            if got_started && got_ready { break; }
        }
    }
    assert!(got_started, "expected NavigationStarted");
    assert!(got_ready, "expected DocumentReady");
}

#[tokio::test]
async fn wait_for_emits_waiting_event() {
    let engine = OxiBrowserEngine::new().await.unwrap();
    let mut events = engine.subscribe_events().unwrap();
    let tab = engine.new_tab().await.unwrap();

    // First navigate, then wait for a non-existent selector
    tab.goto("https://example.com").await.unwrap();
    // Drain ready event
    let _ = events.recv().await;

    let _ = tab.wait_for(".nonexistent", 100).await;  // expected to time out
    match events.recv().await {
        Ok(BrowserEvent::WaitingForSelector { selector, .. }) => {
            assert_eq!(selector, ".nonexistent");
        }
        other => panic!("expected WaitingForSelector, got {other:?}"),
    }
}
```

### Manual smoke test (with oxios)

```bash
# In oxios/
cargo run -- run --json "Find the homepage of oxibrowser and summarize it"

# In a browser:
# Open http://localhost:8080 (oxios-web)
# Watch the timeline — you should see a "browse" tool card with:
#   - Spinner while running
#   - Progress line: "Opening https://…"
#   - Optional second line: "Waiting for `selector` (up to 30s)…"
#   - Third line: "Loaded "…" — 200 · 12 KB · 4 scripts · 245 ms"
#   - Final summary after tool_end
```

---

## 7. Risk analysis

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| `Browser::emit_event` blocks when channel is full | Very low | Low | 32-slot buffer + `let _ = send(...)` — never blocks; we emit ≤4 events per page load |
| Oxios-kernel `#[non_exhaustive]` enum change breaks downstream `match` | Low | Medium | Compile error on missing arm is *helpful*; users get a clear diff to fix |
| BrowseTool default `on_progress` is called before `execute`, but the OxiBrowserEngine events fire during execute — race? | Low | Low | The `ProgressForwarder` is set in `on_progress` *before* `execute`, so no race. We `Drop` the previous callback in the engine to avoid leaks across tool calls |
| `BrowseResult` does not currently carry `total_bytes` or `js_script_count` | Medium | Low | Add these as optional fields with `#[serde(default)]`; fallback to `0` if not tracked. Document as additive. |
| `oxios-web` chat.ts tests don't currently cover `isRunning` lifecycle | Medium | Low | Add tests in same PR |
| OxiBrowserEngine's background event task outlives the engine | Medium | Medium | Cancel via `_event_task` JoinHandle in `close()`. Add test that no event is delivered after `close()` |
| crates.io publish for `oxi-sdk` requires oxios team coordination | High | Medium | Local dev can use `path = "../oxi-sdk"` override. Document the publish order in §9. |
| Sub-resource events missed in v0.12 come back as a request in v0.13 | Low | Low | Documented in §5 P2 — explicit deferral, not omission |

---

## 8. Phased delivery & exit criteria

### Phase 0 — local dev setup (Day 1, no upstream changes)

- [ ] oxibrowser: add `BrowserEvent` enum (4 variants) and `subscribe_events` API
- [ ] oxibrowser: emit events in `Tab::goto` / `wait_for` / `screenshot`
- [ ] oxibrowser: extract `total_bytes` and `js_script_count` if not already in `BrowseResult`
- [ ] oxibrowser: unit tests pass, integration tests (`goto_emits_navigation_and_ready`, `wait_for_emits_waiting_event`) pass
- [ ] Exit: `cargo test -p oxibrowser-core` green; binary `fetch` still works; no measurable perf regression

### Phase 1 — oxi-agent wiring (Day 2-3)

- [ ] oxi-agent: extend `BrowserEngine` trait with `progress_forwarder()` (default impl returns empty forwarder)
- [ ] oxi-agent: implement on `OxiBrowserEngine` (background task draining events → forwarder)
- [ ] oxi-agent: `BrowseTool` overrides `on_progress` to set the engine's forwarder
- [ ] oxi-agent: `cargo test -p oxi-agent` green
- [ ] Exit: when an agent calls `BrowseTool` and the engine emits a `BrowserEvent`, the `AgentEvent::ToolExecutionUpdate { partial_result }` is observed by the `on_event` callback in `agent.run_streaming`

### Phase 2 — oxi-sdk bump (Day 3)

- [ ] oxi-sdk: re-export `BrowserEvent`
- [ ] oxi-sdk: bump to 0.27.0
- [ ] Exit: `cargo build` of a downstream consumer using `oxi-sdk` still works

### Phase 3 — oxios-kernel conversion (Day 4)

- [ ] oxios-kernel: add `KernelEvent::ToolExecutionProgress` variant
- [ ] oxios-kernel: `agent_runtime.rs` handles `AgentEvent::ToolExecutionUpdate` (new match arm)
- [ ] oxios-kernel: `audit_action` mapping
- [ ] oxios-kernel: bump to 1.0.3
- [ ] Exit: `cargo test -p oxios-kernel` green; `KernelEvent` is still `#[non_exhaustive]`

### Phase 4 — oxios-web (Day 5-6)

- [ ] oxios-web: SSE sanitizer handles `ToolExecutionProgress`
- [ ] oxios-web: WS chunk converter emits `tool_progress` chunk
- [ ] oxios-web: rfc015_tests extended with `tool_progress_emits_tool_progress_chunk`
- [ ] oxios-web/web: `StreamChunk` union extended with `tool_progress` variant
- [ ] oxios-web/web: `ChatActivity.tool_call.progress` and `isRunning` fields added
- [ ] oxios-web/web: `chunkToActivity` handles `tool_progress`, sets `isRunning: true` on `tool_start`, clears on `tool_end`
- [ ] oxios-web/web: `ActivityCard` shows `<Loader2 />` spinner + `progress` text while `isRunning`
- [ ] oxios-web/web: stores.test.ts extended
- [ ] Exit: `bun test` green; manual smoke test shows progress in the UI

### Phase 5 — docs & release (Day 7)

- [ ] oxibrowser `CHANGELOG.md`: v0.12.0 entry
- [ ] oxibrowser `docs/ARCHITECTURE.md`: short note on event flow
- [ ] oxios `CHANGELOG.md`: 1.0.3 entry
- [ ] oxios `docs/rfc-015-chat-transparency.md`: mark as extended
- [ ] Publish to crates.io in order: `oxibrowser-core` → `oxi-agent` → `oxi-sdk` → `oxios` (see §9)

**Total: ~1 week for one engineer working top-to-bottom.**

---

## 9. Publishing & coordination

oxios depends on `oxi-sdk` via **crates.io** (`Cargo.toml:84` `oxi-sdk = "0.26.2"`). The publish order matters:

```
1. oxibrowser-core 0.12.0  → crates.io     (additive, no API breaks)
2. oxi-agent 0.27.0        → crates.io     (one new trait method with default impl)
3. oxi-sdk 0.27.0          → crates.io     (re-exports only)
4. oxios 1.0.3             → crates.io     (new KernelEvent variant + WS chunk)
5. oxios-web/dist.zip      → GitHub Release (frontend bundle)
```

For local dev before everything is published, oxios's `Cargo.toml` can be temporarily overridden:

```toml
# oxios/Cargo.toml  (local dev only — revert before commit)
oxi-sdk = { path = "../oxi/oxi-sdk" }
```

**Action items requiring team coordination:**

- [ ] Notify oxi-sdk maintainer (a7garden) about the 0.27.0 release
- [ ] Notify oxios maintainer about the 1.0.3 release and frontend dist update
- [ ] Coordinate on the wire format of `tool_progress` (already a draft in §4.4)

---

## 10. Open questions for the user

1. **Throttling** — given the simplified 4-event enum, a single `goto` produces at most 3 events (NavigationStarted, optional WaitingForSelector, DocumentReady). Throttling is now unnecessary; broadcast's drop-oldest is fine. (No action needed — confirmed by simplification.)

2. **Persistence** — should `ToolExecutionProgress` events be persisted to the session log? If yes, the trajectory_steps schema needs a new `progress_fragments: Vec<String>` field. Default in this plan: **no**, defer to P2. The progress events are ephemeral — they only matter *during* the tool call, and the `output_summary` in `tool_end` already captures the final state.

3. **Backward compat for oxios `chunkToActivity` consumers** — adding a `progress?: string` field to `ChatActivity` is additive. But the `isRunning?: boolean` field changes the default rendering. Are there other consumers of the `tool_call` activity type that this could break? (e.g., export to JSON, mobile app). Default in this plan: `isRunning` is optional and `undefined` is treated identically to `false`.

---

## Appendix A — File-level change summary

| File | Action | LoC |
|------|--------|-----|
| `oxibrowser/crates/oxibrowser-core/src/event.rs` | **new** | 100 |
| `oxibrowser/crates/oxibrowser-core/src/lib.rs` | edit | 2 |
| `oxibrowser/crates/oxibrowser-core/src/browser.rs` | edit | 18 |
| `oxibrowser/crates/oxibrowser-core/src/tab.rs` | edit | 22 |
| `oxibrowser/crates/oxibrowser-core/Cargo.toml` | version bump | 1 |
| `oxi/oxi-agent/src/tools/browse/engine.rs` | edit | 30 |
| `oxi/oxi-agent/src/tools/browse/oxibrowser_backend.rs` | edit | 28 |
| `oxi/oxi-agent/src/tools/browse/browse_tool.rs` | edit | 10 |
| `oxi/oxi-agent/Cargo.toml` | version bump | 1 |
| `oxi/oxi-sdk/src/lib.rs` | edit | 2 |
| `oxi/oxi-sdk/Cargo.toml` | version bump | 1 |
| `oxios/crates/oxios-kernel/src/event_bus.rs` | edit | 12 |
| `oxios/crates/oxios-kernel/src/agent_runtime.rs` | edit | 18 |
| `oxios/crates/oxios-kernel/Cargo.toml` | version bump | 1 |
| `oxios/surface/oxios-web/src/routes/events.rs` | edit | 12 |
| `oxios/surface/oxios-web/src/routes/chat.rs` | edit | 22 |
| `oxios/surface/oxios-web/Cargo.toml` | version bump | 1 |
| `oxios/surface/oxios-web/web/src/types/index.ts` | edit | 4 |
| `oxios/surface/oxios-web/web/src/stores/chat.ts` | edit | 20 |
| `oxios/surface/oxios-web/web/src/components/chat/activity-card.tsx` | edit | 12 |
| `oxios/surface/oxios-web/web/src/__tests__/stores.test.ts` | edit | 30 |
| **Total** | | **≈ 350 LoC** |

(Plus ~120 LoC of tests, +20% buffer = ~500 LoC total. Matches §1 estimate.)

---

## Appendix B — Why not use CDP events directly?

A reasonable alternative is to skip the oxi-agent layer and have oxios spawn an `oxibrowser serve` process, connect to its CDP, and subscribe to `Page.frameNavigated`, `Network.responseReceived`, etc.

**Why we're not doing that in v0.12:**

1. **oxibrowser as a library** is a major design pillar. Spawning a separate process loses the in-process `Browser::new()` integration.
2. **CDP serializes everything as JSON** over WebSocket. The oxi-agent path stays in-process and uses `broadcast::Sender` which is faster and avoids parse overhead.
3. **The library path gives us clean state-transition events** (NavigationStarted, DocumentReady) that map 1:1 to user-visible progress, without forcing us to subscribe to every `Network.*` event and filter.

The CDP path is a fine P2 add-on for users who want to attach Playwright/Puppeteer to an oxios-launched browser. We don't preclude it.
