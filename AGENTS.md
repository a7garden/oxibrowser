# OxiBrowser AGENTS.md

> Convention guide for AI agents working on this codebase.

## Project Overview

OxiBrowser is a headless browser built in pure Rust, designed for AI agents and automation. Inspired by [Lightpanda](https://github.com/lightpanda-io/browser) (Zig), but fully Rust-native. It provides:

- **Headless browsing** — fetch pages, parse HTML, evaluate JavaScript
- **CDP (Chrome DevTools Protocol)** — Puppeteer/Playwright compatible WebSocket server
- **DOM manipulation** — query selectors, DOM mutation (createElement, appendChild, removeChild), text extraction, Markdown conversion
- **CSS text rendering** — ASCII/Unicode DOM→text rendering + PNG screenshot (bitmap font)
- **AI-agent extensions** — OXI CDP domain with `getMarkdown`, `getPageInfo`

The project targets AI agent workflows where a lightweight, fast, embeddable browser is needed without the overhead of Chromium.

> **Implementation status**: See `CHANGELOG.md` for the authoritative, version-tracked feature history.

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                      OxiBrowser                        │
├──────────────────────────────────────────────────────┤
│  CDP Server (WebSocket, Puppeteer/Playwright compat)    │
│  10 domains: Browser, DOM, Fetch, Input, Network,       │
│             OXI, Page, Runtime, Target                  │
├──────────────────────────────────────────────────────┤
│  Browser → Session → Page → Frame                     │
├──────────┬──────────┬──────────────┬─────────────────┤
│  WebAPI  │  Network │  JS Runtime  │  CSS Rendering  │
│  DOM     │  HTTP    │  boa_engine  │  PNG screenshot │
│  Tree    │  Cookies │  ES2024+     │  text→PNG       │
│  Node    │  SSRF    │  persistent  │  font_8x16.bin  │
│  Storage │  robots  │  context     │                 │
├──────────┴──────────┴──────────────┴─────────────────┤
│           html5ever · encoding_rs · reqwest · image    │
└──────────────────────────────────────────────────────┘
```

### Core Hierarchy: Browser → Session → Page → Frame

| Level | Type | Responsibility |
|-------|------|---------------|
| `Browser` | Top-level singleton | Owns sessions, HTTP client, global cookie jar, browser config |
| `Session` | Browsing context group | Owns pages, navigation history, local storage, JS runtime |
| `Page` | Loaded document | Owns root frame, sub-resources, metadata (status, content-type, title), text rendering |
| `Frame` | Document frame | Owns parsed DOM (`Document`), child frames (iframes), raw HTML |

Each level has a unique atomic ID (`BrowserId`, `SessionId`, `PageId`, `FrameId`, `NodeId`) with thread-safe counters.

### JS Runtime

`boa_engine`-based persistent JavaScript runtime:

- **Persistent context** — JS state (variables, functions, closures) survives across `evaluate()` calls
- Runs on a dedicated `std::thread` with `mpsc` channel communication (boa `Context` is `!Send`)
- `TokioJobQueue` for `setTimeout`/`setInterval` timer scheduling
- **JS↔Rust bridges**: fetch channel, localStorage channel, DOM snapshot sync

**Web APIs registered**:

| API | Implementation |
|-----|----------------|
| `document.querySelector/All` | DomSnapshot lookup |
| `document.createElement/createTextNode` | DomMutation creation |
| `document.activeElement` | Returns document.body |
| `document.elementFromPoint(x,y)` | DOM-order approximation |
| `element.appendChild/removeChild` | DomMutation tracking |
| `element.getAttribute/hasAttribute` | Attribute lookup |
| `element.addEventListener/dispatchEvent` | Real JS event system |
| `document.write()` | Appends text node to body |
| `fetch()` | Channel-based HTTP bridge |
| `XMLHttpRequest` | Full XHR with callbacks |
| `localStorage` | Storage interface + Session sync |
| `MutationObserver` | observe/disconnect/takeRecords |
| `setTimeout/setInterval/clearTimeout` | TokioJobQueue timers |
| `atob/btoa` | Base64 encode/decode |
| `URL/URLSearchParams` | url::Url parsing |
| `crypto.getRandomValues` | Pseudo-random bytes |
| `TextEncoder/TextDecoder` | UTF-8 encode/decode |

### CDP Server

The CDP server implements the Chrome DevTools Protocol over WebSocket:

- HTTP endpoints: `/json/version`, `/json` (target discovery)
- WebSocket message dispatch to **10 domain handlers**
- Protocol types: `CdpRequest`, `CdpResponse`, `CdpEvent`
- Event broadcasting: `frameNavigated`, `domContentLoadedEventFired`, `loadEventFired`, `requestWillBeSent`, `responseReceived`, `loadingFinished`, `executionContextCreated`, `consoleAPICalled`
- **OXI domain**: `OXI.getMarkdown`, `OXI.getPageInfo` (AI-agent extensions)

### Network Layer

| Component | Purpose |
|-----------|---------|
| `HttpClient` | reqwest wrapper with cookies, intercept |
| `CookieJar` | Domain-scoped cookie storage |
| `IpFilter` | SSRF prevention — CIDR blocking |
| `RobotStore` | RFC 9309 robots.txt parser |
| `Resource` | Typed resource tracking |
| `InterceptRegistry` | Paused request tracking for Fetch domain |

### CSS Rendering

`crates/oxibrowser-core/src/css/` provides two renderers:

- **render.rs**: ASCII/Unicode text rendering — `render_to_text()`, `render_to_markdown()`
- **screenshot.rs**: PNG image rendering — `text_to_png()` with embedded 8×16 bitmap font (ASCII 32–126)

## Directory Structure

```
oxibrowser/
├── Cargo.toml              # Workspace definition
├── CHANGELOG.md            # Version-tracked feature history
├── AGENTS.md               # This file — conventions & architecture
├── crates/
│   ├── oxibrowser/         # Binary + CLI entry point
│   │   └── src/
│   │       ├── main.rs     # CLI: fetch, serve, version (clap)
│   │       └── lib.rs
│   │   └── tests/
│   │       ├── integration.rs  # Real-website tests (--ignored)
│   │       └── smoke.rs        # Puppeteer/Playwright smoke tests
│   ├── oxibrowser-core/     # Core engine
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── browser.rs
│   │       ├── session.rs
│   │       ├── page.rs          # to_screenshot_png()
│   │       ├── frame.rs
│   │       ├── config.rs
│   │       ├── encoding.rs     # Charset detection + decoding
│   │       ├── error.rs        # CoreError + InterceptedResponse
│   │       ├── js/
│   │       │   ├── mod.rs      # re-exports + input helpers
│   │       │   ├── runtime.rs  # JsRuntime — boa_engine context (4871 lines)
│   │       │   ├── dom_snapshot.rs  # DomSnapshot + DomMutation
│   │       │   ├── input.rs    # js_dispatch_key/mouse/insert_text()
│   │       │   └── job_queue.rs
│   │       ├── css/
│   │       │   ├── mod.rs      # render_to_text + text_to_png re-exports
│   │       │   ├── render.rs   # ASCII/Unicode text renderer
│   │       │   ├── screenshot.rs  # PNG renderer + font data
│   │       │   └── font_8x16.bin  # 8×16 bitmap font (95 chars, 1520 bytes)
│   │       └── network/
│   │           ├── mod.rs       # re-exports
│   │           ├── client.rs    # HttpClient + intercept()
│   │           ├── cookie.rs
│   │           ├── resource.rs
│   │           ├── ip_filter.rs
│   │           ├── robots.rs
│   │           └── intercept.rs  # PausedRequestRegistry + InterceptAction
│   ├── oxibrowser-cdp/        # CDP server
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── server.rs
│   │       ├── session.rs
│   │       ├── protocol.rs
│   │       ├── event.rs
│   │       └── domains/
│   │           ├── mod.rs           # dispatch() + DispatchContext
│   │           ├── browser.rs
│   │           ├── dom.rs
│   │           ├── fetch.rs         # continue/fail/fulfillRequest
│   │           ├── input.rs         # dispatchKey/Mouse/insertText
│   │           ├── network.rs
│   │           ├── oxi.rs
│   │           ├── page.rs         # navigate + captureScreenshot
│   │           ├── runtime.rs
│   │           └── target.rs
│   │   └── tests/e2e.rs       # 23 E2E tests (tokio-tungstenite)
│   └── oxibrowser-webapi/     # DOM
│       └── src/dom/{document,node,tree}.rs
└── docs/                      # Design documents
```

## Code Conventions

### Language

- All code, comments, docs, commit messages: **English**

### Rust

- Edition 2021 (workspace-wide), MSRV: 1.82
- Error handling: `thiserror` for library crates, `anyhow` for binary
- Async: `tokio` throughout, `parking_lot::RwLock` for sync, `tokio::sync::RwLock` for async
- Serialization: `serde` + `serde_json` for CDP wire format
- IDs: Atomic counters (`AtomicU64`/`AtomicU32`)

### Naming

- Crates: `oxibrowser-<component>` (kebab-case)
- Modules: `snake_case`
- Types/traits: `PascalCase`
- Public API: `verb_noun` pattern
- ID types: `PascalCaseId` (`BrowserId`, `SessionId`, `PageId`, `FrameId`, `NodeId`)

### Async Patterns

- `Browser::new()`, `Session::navigate()` etc. are `async`
- `JsRuntime::evaluate()` is `async` (sends command to JS thread via `mpsc` channel)
- `HttpClient::fetch()` is truly async (reqwest)

### Interior Mutability

| Data | Lock | Reason |
|------|------|--------|
| `Browser.sessions` | `parking_lot::RwLock` | Sync access to list |
| `Session` in Browser | `tokio::sync::RwLock` | Async access per session |
| `CookieJar` | `parking_lot::RwLock` in `Arc` | Shared across HttpClient and Session |
| DOM mutations | `parking_lot::RwLock<Vec<DomMutation>>` | Shared between JS thread and async callers |

## Testing Strategy

- Unit tests: `#[cfg(test)] mod tests` within each file
- Integration tests: `tests/` directory per crate
- E2E tests: `crates/oxibrowser-cdp/tests/e2e.rs` (tokio-tungstenite WebSocket client)
- Smoke tests: `crates/oxibrowser/tests/smoke.rs` (Puppeteer process spawn)
- `cargo test --workspace` must pass at every commit
- Real-website integration tests: `--ignored` flag required

## Commit Conventions

```
<type>(<scope>): <description>

Types: feat, fix, refactor, test, docs, chore
Scopes: core, cdp, webapi, cli, docs
```

## Key Principles

1. **Mirrors Lightpanda but Rust-native**: Browser → Session → Page → Frame hierarchy ported from Lightpanda's Zig code.

2. **CDP-compatible**: Wire-compatible with Chrome DevTools Protocol. Puppeteer and Playwright must connect without knowing they're talking to OxiBrowser.

3. **Pure Rust, zero C deps**: `boa_engine` for JS (no V8/SpiderMonkey), `encoding_rs` for charset (no ICU). Single static binary.

4. **AI-agent-first**: Designed for programmatic control via CDP. Optimization targets: startup time, memory usage, CDP response latency.

5. **Thread-safe by default**: All shared state uses `Arc<RwLock>`, `AtomicBool`, `AtomicU64`.

## Key Technical Decisions

### boa_engine for JS runtime

`boa_engine` is pure Rust ES2024+ JavaScript engine — no C dependencies, MIT licensed, lightweight (~1MB compiled). `Context` is `!Send` so JS runs on a dedicated `std::thread` with `mpsc` channels.

### JsRuntime thread architecture

```
main thread (async)          JS thread (sync, std::thread)
┌─────────────┐             ┌──────────────────┐
│ JsRuntime   │──mpsc send─→│ Context (persist)│
│ evaluate()  │             │  register APIs    │
│ set_dom()   │←mpsc recv───│  eval(script)    │
└─────────────┘             └──────────────────┘
```

JS state persists across calls. `TokioJobQueue` bridges `setTimeout`/`setInterval` to tokio's timer wheel.

### JS↔DOM bridge (DomSnapshot)

DOM lives in webapi (main thread), JS lives in boa (JS thread):

1. `Frame`'s `Document` → `DomSnapshot::from_frame()` → serialized → sent to JS thread
2. JS operates via `document.querySelector()`, `createElement()`, etc.
3. Mutations collected as `Vec<DomMutation>` → drained by main thread → applied to real DOM

### JS↔Network bridge

JS `fetch()` and `XMLHttpRequest` use `std::sync::mpsc` channels:
- JS thread sends `FetchRequestMsg` → blocking thread receives → calls `HttpClient` → sends `FetchResponseMsg` back

## Development Guide

### Adding a New CDP Domain

1. Create `crates/oxibrowser-cdp/src/domains/<domain>.rs` with `handle()` function
2. Add `pub mod my_domain` and `"MyDomain" => my_domain::handle()` in `domains/mod.rs`
3. Add tests in `#[cfg(test)]` block

### Adding a New JS Global (Web API)

1. Register in `create_context()` at `crates/oxibrowser-core/src/js/runtime.rs`
2. For async operations: create an `mpsc` channel bridge
3. For DOM operations: add to `DomSnapshot` and `DomMutation` types
4. Test via `JsRuntime::evaluate()` in a unit test

### Adding a New CSS Renderer

1. Add module in `crates/oxibrowser-core/src/css/`
2. Export via `crates/oxibrowser-core/src/css/mod.rs`
3. Wire into `Page::to_screenshot_png()` or create new `Page` method

## Build & Run

```bash
cargo build                    # Build everything
cargo test --workspace         # Run all tests (208 tests)
cargo test --workspace -- --ignored  # Include real-website integration tests
cargo run -- fetch <url>       # Fetch and render a URL
cargo run -- serve              # Start CDP server
cargo run -- version            # Print version
cargo build --release           # Release build
```