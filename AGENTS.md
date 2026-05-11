# OxiBrowser AGENTS.md

> Convention guide for AI agents working on this codebase.

## Project Overview

OxiBrowser is a headless browser built in pure Rust, designed for AI agents and automation. Inspired by [Lightpanda](https://github.com/lightpanda-io/browser) (Zig), but fully Rust-native. It provides:

- **Headless browsing** — fetch pages, parse HTML, evaluate JavaScript
- **CDP (Chrome DevTools Protocol)** — Puppeteer/Playwright compatible WebSocket server
- **DOM manipulation** — query selectors, text extraction, Markdown conversion
- **Servo ecosystem** — html5ever for HTML parsing, with a roadmap for full Servo offscreen rendering

The project targets AI agent workflows where a lightweight, fast, embeddable browser is needed without the overhead of Chromium.

## Architecture

```
┌─────────────────────────────────────────────────┐
│                    OxiBrowser                    │
├─────────────────────────────────────────────────┤
│  CDP Server (WebSocket, Puppeteer/Playwright)    │
├─────────────────────────────────────────────────┤
│  Browser → Session → Page → Frame               │
├──────────┬──────────┬───────────────────────────┤
│  WebAPI  │  Network │  JS Runtime (Servo V8/SM) │
│  DOM     │  HTTP    │  evaluate_javascript()     │
│  CSS     │  WS      │  event loop                │
│  Storage │  Cache   │                            │
├──────────┴──────────┴───────────────────────────┤
│              Servo Engine (Rendering)            │
│         html5ever · cssparser · offscreen        │
└─────────────────────────────────────────────────┘
```

### Core Hierarchy: Browser → Session → Page → Frame

| Level | Type | Responsibility |
|-------|------|---------------|
| `Browser` | Top-level singleton | Owns sessions, HTTP client, global cookie jar, browser config |
| `Session` | Browsing context group | Owns pages, navigation history, local storage, JS runtime |
| `Page` | Loaded document | Owns root frame, sub-resources, metadata (status, content-type, title) |
| `Frame` | Document frame | Owns parsed DOM (`Document`), child frames (iframes), raw HTML |

Each level has a unique atomic ID (`BrowserId`, `SessionId`, `PageId`, `FrameId`) with thread-safe counters.

### CDP Server

The CDP server (`oxibrowser-cdp`) implements the Chrome DevTools Protocol over WebSocket. It provides:
- HTTP endpoints: `/json/version`, `/json` (target discovery)
- WebSocket message dispatch to domain handlers
- Protocol types: `CdpRequest`, `CdpResponse`, `CdpEvent`

### JS Runtime

Dual-mode JavaScript runtime:
- **Stub mode** (default): Minimal expression evaluator (string/number/boolean literals, `console.log`, `document.title`, global variable lookup)
- **Servo mode** (`full-servo` feature, not yet wired): Wraps `servo::WebView::evaluate_javascript()` for real JS execution via SpiderMonkey/V8

### Network Layer

- **HttpClient**: Wraps `reqwest` with cookie injection, redirect following, and configurable pool size
- **CookieJar**: Domain-scoped cookie storage with `store()` / `cookies_for_url()` methods
- **Resource**: Typed resource tracking (`Document`, `Script`, `Stylesheet`, `Image`, `Font`, `XHR`, `Fetch`, `WebSocket`, `Other`)

## Directory Structure

```
oxibrowser/
├── Cargo.toml                  # Workspace definition
├── crates/
│   ├── oxibrowser/             # Binary + CLI entry point (placeholder)
│   ├── oxibrowser-core/        # Core engine: Browser, Session, Page, Frame
│   │   └── src/
│   │       ├── lib.rs          # Re-exports
│   │       ├── browser.rs      # Browser (top-level)
│   │       ├── session.rs      # Session (browsing context)
│   │       ├── page.rs         # Page (loaded document)
│   │       ├── frame.rs        # Frame (DOM + child frames)
│   │       ├── config.rs       # BrowserConfig
│   │       ├── error.rs        # CoreError enum
│   │       ├── js/
│   │       │   ├── mod.rs      # JS runtime abstraction module
│   │       │   └── runtime.rs  # JsRuntime (stub + servo modes)
│   │       └── network/
│   │           ├── mod.rs      # Network module re-exports
│   │           ├── client.rs   # HttpClient (reqwest wrapper)
│   │           ├── cookie.rs   # CookieJar (domain-scoped)
│   │           └── resource.rs # Resource + ResourceType
│   ├── oxibrowser-cdp/         # Chrome DevTools Protocol server
│   │   └── src/
│   │       ├── lib.rs          # Re-exports (CdpServer)
│   │       ├── server.rs       # CdpServer: HTTP endpoints (/json, /json/version)
│   │       ├── session.rs      # CdpSession: per-connection WebSocket dispatch + events
│   │       ├── protocol.rs     # CdpRequest, CdpResponse, CdpEvent, JsonVersion, JsonTarget
│   │       ├── event.rs        # EventSender/EventReceiver (broadcast channel)
│   │       └── domains/
│   │           ├── mod.rs      # dispatch() router + DispatchContext + DomainResult
│   │           ├── browser.rs  # Browser domain (getVersion, close)
│   │           ├── dom.rs      # DOM domain (getDocument, querySelector, ...)
│   │           ├── fetch.rs    # Fetch domain (network interception)
│   │           ├── network.rs  # Network domain (enable/disable, lifecycle events)
│   │           ├── page.rs     # Page domain (navigate, reload, getFrameTree, lifecycle events)
│   │           ├── runtime.rs  # Runtime domain (evaluate, executionContextCreated)
│   │           └── target.rs   # Target domain (createTarget, attachToTarget)
│   │   └── tests/
│   │       └── e2e.rs          # Pure-Rust E2E tests (tokio-tungstenite client)
│   │           ├── network.rs  # Network domain (enable, disable, ...)
│   │           ├── page.rs     # Page domain (navigate, reload, screenshot, ...)
│   │           ├── runtime.rs  # Runtime domain (evaluate, callFunctionOn, ...)
│   │           └── target.rs   # Target domain (attachToTarget, createTarget, ...)
│   └── oxibrowser-webapi/      # DOM and Web API implementations
│       └── src/
│           ├── lib.rs          # Re-exports (Document)
│           ├── dom.rs          # DOM module re-exports
│           └── dom/
│               ├── document.rs # Document: HTML parsing, queries, Markdown
│               ├── node.rs     # Node, NodeId, NodeType
│               └── tree.rs     # Tree: adjacency-list parent/child structure
├── docs/                       # Documentation (to be created)
├── AGENTS.md                   # This file
└── README.md                   # Project overview
```

## Crate Dependency Map

```
oxibrowser (binary)
├── oxibrowser-cdp
│   └── oxibrowser-core
│       └── oxibrowser-webapi
└── oxibrowser-core
    └── oxibrowser-webapi
```

| Crate | Depends On | Purpose |
|-------|-----------|---------|
| `oxibrowser` | `oxibrowser-core`, `oxibrowser-cdp` | Binary entry point, CLI |
| `oxibrowser-core` | `oxibrowser-webapi` | Browser lifecycle, network, JS runtime |
| `oxibrowser-cdp` | `oxibrowser-core` | CDP WebSocket server, protocol dispatch |
| `oxibrowser-webapi` | *(none internal)* | DOM parsing via html5ever, node/tree types |

## Code Conventions

### Language

- All code, comments, docs, commit messages: **English**
- User-facing documentation: English

### Rust

- Edition 2021 (workspace-wide)
- MSRV: current stable Rust
- `#![warn(missing_docs)]` on public crates (planned)
- Error handling:
  - `thiserror` for library crates (`CoreError` enum with `#[derive(Error)]`)
  - `anyhow` for application/binary crate
  - `Result<T>` type alias in each crate (`crate::error::Result<T>`)
- Async: `tokio` runtime throughout
  - `parking_lot::RwLock` for sync interior mutability (browser sessions list, cookie jar)
  - `tokio::sync::RwLock` for async-guarded data (Session in Browser)
- Serialization: `serde` + `serde_json` for wire formats (CDP messages), no TOML config file yet
- IDs: Atomic counters (`AtomicU64` / `AtomicU32`) for thread-safe unique IDs

### Naming

- Crates: `oxibrowser-<component>` (kebab-case)
- Modules: `snake_case`
- Types/traits: `PascalCase`
- Public API: verb_noun pattern (`new_session`, `fetch_text`, `query_selector`)
- ID types: `PascalCaseId` (`BrowserId`, `SessionId`, `PageId`, `FrameId`, `NodeId`)

### Error Handling Pattern

Each crate defines its own error enum:

```rust
// In oxibrowser-core
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("navigation failed: {0}")]
    NavigationFailed(String),
    // ...
}
pub type Result<T> = std::result::Result<T, CoreError>;
```

CDP domain handlers return `DomainResult = Result<Option<Value>, CdpError>`.

External error types are converted via `impl From<...> for CoreError`.

### Async Patterns

- `Browser::new()`, `Session::navigate()` etc. are async
- `Frame::from_html()` is async (for future servo integration) but currently synchronous internally
- `JsRuntime::evaluate()` is async (for future servo mode)
- `HttpClient::fetch()` is truly async (reqwest)

### Interior Mutability

| Data | Lock Type | Reason |
|------|-----------|--------|
| `Browser.sessions` | `parking_lot::RwLock<Vec<Arc<tokio::sync::RwLock<Session>>>>` | Sync access to session list, async access to individual sessions |
| `CookieJar` | `parking_lot::RwLock` inside `Arc` | Shared across HttpClient and Session |
| `Browser.closed` | `AtomicBool` | Simple flag, no contention |
| DOM mutation tracking | `dom_version: u64` | Incremented on any DOM mutation |

### Serialization

- CDP protocol types use `#[derive(Serialize, Deserialize)]`
- `CdpRequest`: `Deserialize` only (incoming)
- `CdpResponse`, `CdpEvent`, `JsonVersion`, `JsonTarget`: `Serialize` only (outgoing)
- `#[serde(rename_all = "camelCase")]` for CDP wire format
- `#[serde(skip_serializing_if = "Option::is_none")]` to minimize wire size

## Testing Strategy

- Unit tests in `#[cfg(test)] mod tests` within each file
- Integration tests in `tests/` directory per crate
- `cargo test --workspace` must pass at every commit
- DOM parsing tests: use `Document::parse()` with known HTML, assert node structure
- CDP tests: construct `CdpRequest` JSON, call `dispatch()`, assert `CdpResponse`
- Network tests: mock HTTP responses via `mockito` or similar (planned)

### Key Test Scenarios

| Component | Test Focus |
|-----------|------------|
| `Document::parse()` | HTML parsing correctness, edge cases (malformed HTML, empty input) |
| `Tree` | Parent/child relationships, traversal (DFS, BFS) |
| `Node` | Type checks, attribute access |
| `CookieJar` | Store/retrieve, domain isolation |
| `JsRuntime` (stub) | Literal evaluation, console.log, globals |
| CDP `dispatch()` | Domain routing, error codes, unknown domain |
| `Browser` lifecycle | Create → new_session → close, double-close, max_sessions |
| `Session` navigation | Navigate → go_back → go_forward → reload |

## Commit Conventions

```
<type>(<scope>): <description>

Types: feat, fix, refactor, test, docs, chore
Scopes: core, cdp, webapi, cli, docs
```

Examples:
```
feat(cdp): implement Page.navigate domain handler
fix(core): handle empty HTML input in Frame::from_html
refactor(webapi): optimize Tree::traverse_dfs to use iterative approach
test(cdp): add dispatch tests for all six CDP domains
docs: update ARCHITECTURE.md with session lifecycle diagram
```

## Key Principles

1. **Mirrors Lightpanda but Rust-native:** The Browser → Session → Page → Frame hierarchy is directly inspired by Lightpanda's `Browser.zig` / `Session.zig` / `Page.zig` / `Frame.zig`. We port the architecture, not the code.

2. **Servo-powered:** Use Servo ecosystem crates (html5ever, markup5ever, string_cache) for HTML parsing. Full Servo rendering integration is the roadmap goal.

3. **CDP-compatible:** The CDP server must be wire-compatible with Chrome DevTools Protocol 1.3. Puppeteer and Playwright must be able to connect and perform basic operations without knowing they're talking to OxiBrowser.

4. **No reimplementation:** Use `reqwest` for HTTP, `html5ever` for parsing, `tokio-tungstenite` for WebSocket, `serde_json` for serialization. Don't reinvent the wheel.

5. **AI-agent-first:** Designed for programmatic control via CDP, not human interactive browsing. Optimization targets: startup time, memory usage, CDP response latency.

6. **Progressive enhancement:** Stub implementations (JS runtime, rendering) are acceptable for initial scaffolding. Each stub must have a clear path to the real implementation (servo mode).

7. **Thread-safe by default:** All shared state uses appropriate synchronization (`Arc<RwLock<...>>`, `AtomicBool`, `AtomicU64`).

## Development Guide

### Adding a New CDP Domain

1. **Create the domain file** at `crates/oxibrowser-cdp/src/domains/<domain>.rs`:

```rust
//! <Domain> domain implementation.

use super::DomainResult;
use crate::protocol::CdpError;
use serde_json::Value;

/// Handle a <domain> method.
pub fn handle(method: &str, params: Option<Value>) -> DomainResult {
    match method {
        "methodA" => method_a(params),
        "methodB" => method_b(params),
        _ => Err(CdpError {
            code: -32601,
            message: format!("unknown method: <domain>.{}", method),
        }),
    }
}

fn method_a(params: Option<Value>) -> DomainResult {
    // Implementation
    Ok(Some(serde_json::json!({ "result": "value" })))
}

fn method_b(params: Option<Value>) -> DomainResult {
    // Implementation
    Ok(None)
}
```

2. **Register in dispatcher** at `crates/oxibrowser-cdp/src/domains/mod.rs`:

```rust
pub mod my_domain;  // Add module declaration

// In dispatch():
match domain {
    // ... existing domains ...
    "MyDomain" => my_domain::handle(method_name, params),
    _ => Err(CdpError { ... }),
}
```

3. **Add tests** in a `#[cfg(test)] mod tests` block at the bottom of the domain file.

4. **Document** in `docs/CDP.md`.

### Adding a New WebAPI Type

1. **Create the type file** at `crates/oxibrowser-webapi/src/dom/<type>.rs` or a new sub-module.

2. **Define the type** using `NodeType` variants or new structs:

```rust
/// A new WebAPI type.
pub struct MyType {
    // fields
}

impl MyType {
    pub fn new() -> Self { ... }
}
```

3. **Register the module** in `crates/oxibrowser-webapi/src/dom.rs`:

```rust
pub mod my_type;
pub use my_type::MyType;
```

4. **Ensure `Document` can interact with it** if the type is DOM-related (add query methods, mutation methods as needed).

### Adding a New DOM Operation

1. **Identify the right layer:**
   - **Tree structure** (parent/child, traversal) → `tree.rs`
   - **Node data** (attributes, type checks) → `node.rs`
   - **Document-level queries** (CSS selectors, text extraction) → `document.rs`

2. **Add the method** to the appropriate type.

3. **If mutation:** increment `dom_version` in the containing `Frame` (via `Frame::document_mut()`).

4. **Add tests** for the operation with known HTML fixtures.

### Adding a New Network Feature

1. **Core networking** → `crates/oxibrowser-core/src/network/`
2. **Use `HttpClient`** as the entry point for all HTTP operations.
3. **Cookie handling** → `CookieJar` (add methods as needed).
4. **Resource tracking** → `Resource` / `ResourceType` (add variants as needed).

### Adding a Browser Config Option

1. **Add field** to `BrowserConfig` in `config.rs`.
2. **Set default** in `Default::default()`.
3. **Use in** `HttpClient::new()`, `Browser::new()`, or `Session` methods as appropriate.
4. **Document** the new option.

## Build & Run

```bash
cargo build                    # Build everything
cargo test --workspace         # Run all tests
cargo run                      # Run oxibrowser (when binary is wired)
cargo build --release          # Release build
```

### Feature Flags

| Flag | Crate | Description |
|------|-------|-------------|
| `full-servo` | `oxibrowser-core` | Enable Servo engine for real JS execution and rendering (not yet implemented) |

## Current Implementation Status

| Component | Status | Notes |
|-----------|--------|-------|
| `Browser` | ✅ Complete | Lifecycle, sessions, cookie jar |
| `Session` | ✅ Complete | Navigation, history, local storage |
| `Page` | ✅ Complete | HTML loading, title extraction, resources |
| `Frame` | ✅ Complete | DOM parsing, child frames, queries, sub-resource extraction |
| `Document` | ✅ Complete | html5ever parsing, CSS selectors, Markdown, resource URL extraction |
| `Tree` | ✅ Complete | Adjacency list, DFS/BFS traversal |
| `Node` | ✅ Complete | Type variants, attribute access |
| `JsRuntime` | ✅ Stub | Literal evaluation only; servo mode planned |
| `HttpClient` | ✅ Complete | reqwest wrapper with cookies |
| `CookieJar` | ✅ Complete | Domain-scoped storage |
| `Resource` | ✅ Complete | Typed resource tracking |
| `BrowserConfig` | ✅ Complete | Timeout, viewport, TLS, pool size |
| `CoreError` | ✅ Complete | Typed error variants with `From` impls |
| CDP Protocol types | ✅ Complete | CdpRequest/Response/Event, JsonVersion/Target |
| CDP Event Broadcasting | ✅ Complete | EventSender/EventReceiver with atomic flags |
| CDP DispatchContext | ✅ Complete | Session + EventSender in one context |
| CDP Domain dispatch | ✅ Complete | Router dispatches to 7 domain handlers |
| CDP Server | ✅ Complete | HTTP endpoints + WebSocket upgrade (RFC 6455) |
| CDP Session | ✅ Complete | Per-connection dispatch loop with event forwarding |
| CDP Domain handlers | ✅ Complete | Browser, DOM, Fetch, Network, Page, Runtime, Target |
| CDP Page events | ✅ Complete | frameNavigated, domContentLoadedEventFired, loadEventFired |
| CDP Runtime events | ✅ Complete | executionContextCreated, consoleAPICalled |
| CDP Network events | ✅ Complete | requestWillBeSent, responseReceived, loadingFinished |
| CDP Fetch domain | ✅ Complete | enable/disable/continueRequest/failRequest/fulfillRequest |
| E2E Test Suite | ✅ Complete | 15 pure-Rust E2E tests via tokio-tungstenite |
| Binary / CLI | ✅ Complete | `fetch`, `serve`, `version` subcommands via clap |
| Servo rendering | 🔲 Planned | Offscreen rendering pipeline |
| Servo JS engine | 🔲 Planned | Real JS execution via servo feature flag |
