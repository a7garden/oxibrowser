# OxiBrowser v0.5 Roadmap

> Pure-Rust headless browser for AI agents. Built with boa_engine, html5ever, reqwest, tokio.
> Version 0.2.0 (v0.4 milestone). Target: v0.5 production-ready.

## Vision

OxiBrowser aims to be the **smallest, fastest, embeddable headless browser** for AI agent workflows. Not a Chromium replacement — a purpose-built tool for programmatic page access, JS evaluation, and DOM manipulation at scale. Pure Rust means no C dependencies, no V8, no system browser required. Every binary is self-contained.

**Target users**: AI agents that need to fetch, parse, and interact with web pages programmatically. Puppeteer/Playwright-compatible via CDP.

---

## What's Done (v0.4)

### Core Engine
| Component | Status | Notes |
|-----------|--------|-------|
| Browser / Session / Page / Frame hierarchy | ✅ | Atomic IDs, history, local/session storage |
| JsRuntime (boa_engine) | ✅ | Real ES2024+ JS, no C deps, Context on dedicated thread |
| setTimeout / setInterval | ✅ | TokioJobQueue microtask + timer heap |
| fetch() HTTP | ✅ | Channel-based JS→HttpClient bridge, Response.text()/.json() |
| localStorage / sessionStorage | ✅ | Bidirectional sync JS↔Session |
| URL, btoa/atob, crypto.getRandomValues | ✅ | Full URL class with accessor properties |
| TextEncoder / TextDecoder | ✅ | |
| EventTarget (addEventListener/dispatchEvent) | ✅ | Real implementation |
| document.write() | ✅ | Append HTML content |
| DOM mutation (createElement, appendChild) | ✅ | |
| XMLHttpRequest | ✅ | |
| CookieJar | ✅ | Domain-scoped storage |
| IP filter (RFC1918) | ✅ | 13 private CIDR ranges |
| robots.txt parser | ✅ | RFC 9309 compliant |

### CDP Server
| Domain | Methods | Events |
|--------|---------|--------|
| Browser | getVersion, close | |
| DOM | getDocument, querySelector, querySelectorAll, getAttributes | |
| Fetch | enable, disable, fulfillRequest, continueRequest, failRequest | requestPaused |
| Input | dispatchKeyEvent, dispatchMouseEvent, insertText, imeSetComposition | |
| Network | getAllCookies, setCookie, deleteCookies, getResponseBody | requestWillBeSent, responseReceived, loadingFinished |
| Page | navigate, reload, getFrameTree | frameNavigated, domContentLoaded, loadEventFired |
| Runtime | evaluate, enable | executionContextCreated, consoleAPICalled |
| Target | createTarget, attachToTarget, closeTarget | targetCreated, targetDestroyed |
| OXI | getMarkdown, getPageInfo | |

### Tests & Tooling
| Item | Count | Notes |
|------|-------|-------|
| Unit tests | 152 | core + cdp + webapi |
| E2E tests | 22 | CDP WebSocket integration |
| Smoke tests | 3 | Puppeteer-equivalent workflow |
| Total | 205 | All passing |

**Codebase**: ~13,400 LOC across 4 crates.

---

## What's Left: v0.5 Priorities

### P0 — Production Readiness (Must have)

#### P0.1: Error Recovery & Crash Safety
**What**: The browser must not panic on malformed input. All HTTP failures, parse errors, and JS exceptions must be handled gracefully.

**Why**: AI agents will feed arbitrary HTML, JS, and URLs. A single panic crashes the entire process.

**Implementation**:
- Wrap all `unwrap()` / `expect()` in network/parsing paths with `?` or `Result` handling
- Add `Result` return types to `Session::navigate()`, `Frame::from_html()`, `JsRuntime::evaluate()`
- Add `panic = "unwind"` to `Cargo.toml` `[profile.dev]` for debug builds
- Add `tracing::error` for all error paths (don't just silently swallow)

**Files**: `session.rs`, `frame.rs`, `runtime.rs`, `client.rs`

**Acceptance**: `cargo test --workspace` passes with zero panics under test. No `unwrap()` in public API paths.

---

#### P0.2: Resource Cleanup & Graceful Shutdown
**What**: All spawned threads, tokio tasks, and mpsc channels must be properly cleaned up on Session::close() and Browser::close().

**Why**: Without cleanup, leaked handles accumulate over time. AI agents create/destroy many sessions.

**Implementation**:
- `Session::close()` must drop: `js_runtime` thread, `fetch_task` JoinHandle, `local_storage_task` JoinHandle
- `Browser::close()` must drop all session handles
- Add `closed: AtomicBool` flag (already exists) to prevent double-close
- Add `Shutdown` channel (`tokio::sync::oneshot`) to signal JS thread to exit
- Verify no `Arc::clone()` references leak after `Session::close()`

**Files**: `session.rs`, `runtime.rs`, `browser.rs`

**Acceptance**: `Session::close()` called multiple times does not panic. No leaked threads after 1000 session cycles.

---

#### P0.3: Console Event Emission (CDP)
**What**: `console.log`, `console.error`, `console.warn`, `console.info` from JS must emit `Runtime.consoleAPICalled` events to CDP clients.

**Why**: Puppeteer scripts use `page.on('console', ...)` to capture JS output. Without this, debugging is impossible.

**Implementation**:
- In `js_thread_loop` (runtime.rs), `console_log_fn` closure must send a message to the CDP event channel
- This requires bridging from the JS thread (sync, std::thread) to the CDP event system (tokio, async)
- Approach: `EventSender` stored in `JsRuntime` as `Arc<RwLock<Option<EventSender>>>`. JS thread clones and writes.
- Or: Use `crossbeam-channel` (sync mpsc) from JS thread → tokio task → CDP events

**Files**: `runtime.rs` (console_log_fn), `event.rs` (EventSender), `session.rs` (wiring)

**Acceptance**: E2E test `Runtime.evaluate("console.log('hello')")` receives `Runtime.consoleAPICalled` event with `"hello"` text.

---

#### P0.4: Fetch interception real integration
**What**: `Fetch.requestPaused` events must be emitted before HTTP requests, and `Fetch.fulfillRequest` / `continueRequest` must actually affect the response.

**Why**: Playwright's `page.route()` API depends on this. Without it, request interception is stub-only.

**Implementation**:
- `Session::navigate()` and `fetch()` channel must check `fetch_interceptor` patterns BEFORE making HTTP requests
- When a pattern matches: emit `Fetch.requestPaused` event, store request as "paused" in Session
- When client calls `Fetch.fulfillRequest`: store mock response, next matching request uses mock
- When client calls `Fetch.continueRequest`: remove mock, let request proceed
- `Fetch.requestPaused` needs: `requestId`, `url`, `method`, `headers`, `postData`, `resourceType`, `frameId`

**Files**: `session.rs` (FetchInterceptor), `fetch.rs` (emit_request_paused), `client.rs` (intercept hook)

**Acceptance**: E2E test: Fetch.enable("*.js") → Page.navigate → assert `Fetch.requestPaused` event received with requestId.

---

### P1 — CDP Completeness (Should have)

#### P1.1: Page.screenshot (PNG capture)
**What**: `Page.captureScreenshot` returns PNG image data of the current viewport.

**Why**: Puppeteer's `page.screenshot()` is one of the most-used APIs.

**Implementation**:
- Use `image = "0.25"` crate (already in deps) to encode RGB data as PNG
- For now: return a minimal 1×1 black PNG as stub — full rendering is P2
- Store viewport size in `Session` / `BrowserConfig` (already has viewport fields)
- CDP: `Page.captureScreenshot` → encode frame buffer → return base64 PNG

**Files**: `page.rs` (CDP handler), `page.rs` (core), `config.rs`

**Acceptance**: `Page.captureScreenshot` returns valid PNG (can use minimal placeholder for v0.5).

---

#### P1.2: Network.getResponseBody real capture
**What**: `Network.getResponseBody` must return the actual HTTP response body for any tracked request.

**Why**: Playwright's `response.body()` depends on this.

**Implementation**:
- Already implemented in Session: `CapturedResponse` + `store_response_body()` + `get_response_body()`
- `Session::navigate()` already stores body after fetch
- Need to also capture bodies from `fetch()` channel (JS fetch API)
- In `handle_fetch_requests()` (session.rs), after getting HTTP response, store body with requestId
- Store by URL hash as requestId, or generate UUID per request

**Files**: `session.rs` (capture in handle_fetch_requests), `fetch.rs` (response capture)

**Acceptance**: `Network.getResponseBody` for a navigated page returns non-empty body matching the HTML.

---

#### P1.3: Session storage persistence
**What**: `localStorage` and `sessionStorage` must persist across navigations within the same session.

**Why**: Web apps depend on storage surviving page loads.

**Implementation**:
- Currently: `SetPageUrl` re-registers localStorage with empty map (clears on nav)
- Fix: Remove the "clear on navigation" behavior. localStorage should only clear when explicitly cleared or session closed.
- Keep the `RefCell<HashMap>` in JS runtime as-is. Remove the `register_local_storage(&mut ctx, empty, ...)` on `SetPageUrl`.
- Test: Set localStorage in eval → navigate → get localStorage in new eval → value preserved.

**Files**: `runtime.rs` (SetPageUrl handler), `session.rs`

**Acceptance**: E2E test: `Runtime.evaluate("localStorage.setItem('k','v')")` → `Page.navigate` → `Runtime.evaluate("localStorage.getItem('k')")` returns `"v"`.

---

#### P1.4: Multiple session support in CDP
**What**: CDP Target domain must properly track multiple sessions/pages. `Target.createTarget` creates a new session.

**Why**: Puppeteer creates one page per target. Multi-tab workflows need this.

**Implementation**:
- `Browser.new_session()` already creates isolated sessions
- `Target.createTarget` calls `browser.new_session()` → returns CDP target info
- `Target.closeTarget` calls `browser.close_session(session_id)`
- Events: `Target.targetCreated`, `Target.targetDestroyed`, `Target.targetInfoChanged`
- WebSocket path must include session-specific target ID (not just "main")

**Files**: `target.rs`, `browser.rs`, `server.rs` (WebSocket routing)

**Acceptance**: E2E: Create two targets → navigate each to different URLs → each has independent history/storage.

---

### P2 — Web Platform (Nice to have)

#### P2.1: Complete JS global object
**What**: Implement missing `window` globals: `Math`, `JSON`, `Date`, `Array`, `Object`, `String`, `Number`, `Boolean`, `Promise`, `Symbol`, `JSON.stringify/parse`, `Math.random/floor/ceil/abs`, `Date.now/getTime`.

**Why**: Most web app JS depends on these. Without them, many pages fail to initialize.

**Implementation**:
- Each global is a `NativeFunction` or `JsObject` registered via `context.register_global_callable()` / `register_global_property()`
- `Math`: `Math.random()`, `Math.floor()`, `Math.ceil()`, `Math.abs()`, `Math.max()`, `Math.min()`, `Math.pow()`, `Math.sqrt()`
- `JSON`: `JSON.stringify()`, `JSON.parse()` (already partially implemented via serde_json)
- `Date`: `Date.now()`, `Date.prototype.getTime()`
- `Array`: `Array.isArray()`
- `Symbol`: `Symbol.for()`, `Symbol.keyFor()`
- `Promise`: Constructor stub (can defer full implementation)

**Files**: `runtime.rs` (globals section), `js/mod.rs`

**Acceptance**: `evaluate("JSON.stringify({a:1})")` returns `"{\"a\":1}"`. `evaluate("Math.random()")` returns a number.

---

#### P2.2: History API (back/forward)
**What**: Implement `history.pushState()`, `history.replaceState()`, `history.back()`, `history.forward()`, `history.go()`, and `window.onpopstate`.

**Why**: SPAs (React, Vue, etc.) use History API for routing. Without it, navigation within SPAs doesn't work.

**Implementation**:
- `history` object registered in JS runtime as global
- `pushState(state, title, url)`: adds URL to Session.history, updates current URL
- `replaceState`: updates current history entry (no new entry)
- `back()` / `forward()`: moves history_index, triggers `popstate` event
- `go(delta)`: moves history_index by delta
- `onpopstate` handler fires when `back()`/`forward()`/`go()` changes URL

**Files**: `runtime.rs`, `session.rs` (history field already exists)

**Acceptance**: `evaluate("history.pushState({},'', '/page2'); history.length")` returns updated length.

---

#### P2.3: Location / Navigation API
**What**: Implement `window.location` (href, pathname, search, hash, assign, replace, reload) and `window.location.href` setter.

**Why**: Most web apps use `window.location` for navigation.

**Implementation**:
- `location` as global object with accessor properties: `href`, `protocol`, `host`, `hostname`, `port`, `pathname`, `search`, `hash`, `origin`
- `location.assign(url)` → `Session::navigate(url)`
- `location.replace(url)` → `Session::navigate(url)` (no history entry)
- `location.reload()` → `Session::reload()`
- `location.href = url` setter → navigate

**Files**: `runtime.rs`

**Acceptance**: `evaluate("location.href")` returns current URL. `evaluate("location.href = 'http://example.com/'")` triggers navigation.

---

#### P2.4: DOM event system real implementation
**What**: `addEventListener`, `removeEventListener`, `dispatchEvent` must actually fire event handlers. Event types: click, input, change, submit, load, error.

**Why**: Web apps use DOM events for interactivity. Without firing handlers, clicking buttons doesn't work.

**Implementation**:
- Already have: `EventTarget` with `addEventListener`, `removeEventListener`, `dispatchEvent`
- Need: `click()` method on elements that fires `click` event
- Need: Fire `load` event when page finishes loading
- Need: Fire `DOMContentLoaded` event when DOM is parsed
- Event dispatch must call all registered listeners in order
- `event.target` must be set to the element, `event.type` to event name

**Files**: `runtime.rs` (click method), `page.rs` (event firing on nav completion)

---

### P3 — Performance & Polish

#### P3.1: Binary size optimization
**What**: Reduce release binary size from current ~15MB to target ~8MB.

**Why**: Embeddable browsers should be small for AI agent deployment.

**Implementation**:
- `opt-level = "z"` in `[profile.release]` for size
- `lto = true` for linker optimization
- `codegen-units = 1` for better optimization
- Strip debug symbols: `strip = true`
- Feature gate unused dependencies (e.g., only enable `reqwest/json` features)
- Evaluate `boa_engine` size: can we disable unused features?

**Files**: `Cargo.toml` (profiles), `Cargo.toml` (deps features)

---

#### P3.2: Startup time profiling
**What**: Measure and optimize cold startup time (binary launch → ready to serve CDP).

**Why**: AI agents spawn many browser instances. Sub-100ms startup matters.

**Implementation**:
- Add benchmark: `cargo bench --bench core_bench` with startup timer
- Profile: which part is slowest? (boa_engine init? HTTP client? CDP server?)
- Reduce boa_engine init time: lazy context creation, defer until first eval
- Use `tracing` spans to identify hot paths

**Files**: `benches/core_bench.rs`, `runtime.rs` (init), `browser.rs` (init)

---

#### P3.3: Memory usage profiling
**What**: Measure per-session memory overhead. Target: <50MB per session.

**Why**: AI agents may run hundreds of sessions. Memory efficiency matters.

**Implementation**:
- Profile with `valgrind` or `tracy` (or `dhat-rs` for Rust heap profiling)
- Identify: which allocations are largest? (boa_context, DOM tree, network buffer)
- Optimize: reuse buffers, cap HTML parsing memory, lazy-load subframes

**Files**: `runtime.rs`, `frame.rs`, `client.rs`

---

## Open Questions

### Q1: Multi-process or single-process?
Current design is single-process with async sessions. Lightpanda uses a separate render process. Do we need multi-process for isolation, or is single-process sufficient for AI agent use cases?

**Decision**: Single-process for v0.5. Multi-process adds complexity (IPC, crash isolation). AI agents typically run one browser per task anyway.

### Q2: Rendering — do we need it?
For AI agent use cases, rendering (visual layout) is rarely needed. What matters is DOM access, JS execution, and network interception. Should we invest in Servo rendering at all?

**Decision**: Defer Servo rendering to v0.6+. For v0.5, focus on DOM manipulation and JS execution quality. `Page.captureScreenshot` can return a placeholder PNG.

### Q3: WASM compatibility?
Should we support OxiBrowser running in WASM environments (for AI agents deployed on edge)?

**Decision**: Not in v0.5. boa_engine supports WASM but the networking layer (reqwest) doesn't. Revisit after v0.5 if there's demand.

### Q4: Cookie format compatibility with browsers
Should `Network.setCookie` / `getAllCookies` produce output that is wire-compatible with Chrome's CDP cookie format (so cookies set by OxiBrowser are visible in a real Chrome session on the same machine)?

**Decision**: No. Cookies are session-scoped and stored in CookieJar. No system-level cookie persistence for v0.5.

### Q5: CDP protocol version
We're at CDP 1.3. Should we target 1.4/1.5 for newer features (webauthn, storage bucket, attestation)?

**Decision**: Stay at 1.3 for v0.5. Puppeteer and Playwright still primarily use 1.3. Newer protocol features can wait.

---

## Timeline (v0.5)

```
Month 1 (P0):
├── P0.1 Error Recovery      ← Start here, foundation
├── P0.2 Graceful Shutdown
├── P0.3 Console Events     ← Immediate value for AI agent debugging
└── P0.4 Fetch Interception

Month 2 (P1):
├── P1.1 Page.screenshot
├── P1.2 getResponseBody capture
├── P1.3 Storage persistence
└── P1.4 Multi-session CDP

Month 3 (P2 + P3):
├── P2.1 Complete JS globals
├── P2.2 History API
├── P2.3 Location API
├── P2.4 DOM event firing
├── P3.1 Binary size
├── P3.2 Startup profiling
└── P3.3 Memory profiling
```

**Total estimated**: ~3 months for v0.5 production-ready.

---

## Metrics

| Metric | Current | Target (v0.5) |
|--------|---------|---------------|
| LOC | ~13,400 | ~16,000 |
| Tests | 205 | 280+ |
| Binary size (release) | ~15MB | ~8MB |
| Startup time | unknown | <200ms |
| Memory/session | unknown | <50MB |
| CDP domains | 9 | 12 |
| JS globals | ~30 | ~60 |
| E2E coverage | 22 | 40 |

---

## Reference

- CDP Protocol: https://chromedevtools.github.io/devtools-protocol/
- Puppeteer API: https://pptr.dev/api/puppeteer
- Playwright API: https://playwright.dev/docs/api/class-page
- boa_engine docs: https://docs.rs/boa_engine/latest/boa_engine/
- Lightpanda (reference): https://github.com/lightpanda-io/browser