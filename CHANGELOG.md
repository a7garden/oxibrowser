# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.17.0] - 2026-07-11

### Added

- **HTML serializer** (`crates/oxibrowser-core/src/js/dom_serializer.rs`) — pure-Rust DOM-to-HTML serialization. Void elements self-close, attributes are HTML-escaped, text/comment nodes handled. `serialize_node` and `serialize_children` are the public API; 12 unit tests cover elements/text/comments/void/attrs/document/unknown types.
- **`Element.outerHTML` getter** — reads the serialized tag + attributes + children. Read-only per spec.
- **`innerHTML` setter rewires to a real parser** — assignments now go through `DomSnapshot::set_inner_html`, which calls `oxibrowser_webapi::Document::parse` to parse the fragment, removes the target's old children via `remove_subtree`, and inserts the new subtree via `insert_subtree` (DFS pre-order, fresh node ids, proper parent links). `rebuild_indices` is called after so id/class/tag indices pick up the new nodes and `querySelector` / `getElementById` can find them on the next read.
- **Event constructor init dictionaries** — `new MouseEvent('click', { clientX, clientY, button, ctrlKey, ... })` now copies the init dict onto the event object. Same for `KeyboardEvent`, `FocusEvent`, `Event`, and `DragEvent` (extends MouseEvent + `dataTransfer`).
- **`Event.prototype` methods on every event instance** — `preventDefault`, `stopPropagation`, `stopImmediatePropagation` are set as own properties on each event object so they resolve without depending on the JS class hierarchy.
- **`dispatchEvent` sets `event.target` and `event.currentTarget`** before firing listeners; returns `!defaultPrevented`; respects `stopImmediatePropagation` between callbacks.
- **Event bubbling** — when `event.bubbles === true` and `stopPropagation` wasn't called, dispatch walks the DomSnapshot parent chain and fires ancestor listeners found via the new thread-local `LISTENER_REGISTRY` (keyed by `node_id` so listeners registered through any element-object instance are reached). `event.currentTarget` is updated to the current ancestor.
- **`requestAnimationFrame` / `cancelAnimationFrame`** — proper implementation via `TokioJobQueue::schedule_timer` with a 16 ms deadline. The callback receives a `DOMHighResTimeStamp` (ms since `UNIX_EPOCH`); `cancelAnimationFrame` uses the timer's handle id and `cancel_timer`.
- **`Element.innerText`** — read-only alias for `textContent`.
- **`performance` global** — registered as a standalone global (`window.performance === performance`) alongside the existing `window.performance` accessor. Provides `now()` returning `ms since UNIX_EPOCH`.
- **`Response` improvements** — `__body` is stored as a hidden property on the Response object. `text()`, `json()`, and `arrayBuffer()` all read from `this.__body` (not from a captured closure) and return `Promise.resolve(...)`. `arrayBuffer()` returns a `Uint8Array`. `bodyUsed` is exposed as a property (still hardcoded to `false`).
- **`fetch` options `headers`** — common header keys (`content-type`, `accept`, `authorization`, `user-agent`, `cookie`) from the init dict are now read and forwarded to the HTTP client. Previously silently dropped.
- **CDP `Input.dispatchMouseEvent` multi-event sequence** — `mousePressed` fires `mousedown`; `mouseReleased` fires `mouseup` + `click` (left button only); `mouseMoved` fires `mousemove`. Matches real-browser behavior.
- **CDP `Input.dispatchDragEvent`** — real implementation that evaluates `js_dispatch_drag_event(x, y, event_type)` and dispatches a `DragEvent` on the element at the point. Previously a no-op.
- **`js_dispatch_drag_event` JS code generator** — used by both the CDP handler and the Tab drag API.

### Fixed

- **SSRF filter now scheme-aware** — `check_url_ssrf` short-circuits to `allow` for any non-`http`/`https` scheme (`about:`, `data:`, etc.). Previously, `about:blank` was rejected because the filter tried to resolve the hostname "blank".
- **`about:blank` navigation support** — `Session::navigate` routes `about:` URLs to a new `navigate_about` that builds an empty page from a minimal HTML template. The CDP server's default URL is `about:blank` and now works.

## [Unreleased]

## [0.16.0] - 2026-06-26

### Added

- **Stealth/bot-detection layer** — `ChallengeDetector` for Cloudflare, Turnstile, reCAPTCHA, hCaptcha challenge detection (`challenge.rs`). Automatic retry with clearance cookie detection. Classifies challenges as NonInteractive (solved by retry), Interactive (needs human), or Blocked.
- **Extraction engine** (`extract.rs`) — structured HTML-to-markdown extraction with link collection, metadata parsing (`og:title`, `og:description`, `og:image`, `twitter:card`, canonical URL), and content normalization.
- **JS V8 parity bootstrap** — 818-line bootstrap script in `runtime.rs` simulating real V8 browser globals: `window.navigator` (platform, languages, hardwareConcurrency, maxTouchPoints, deviceMemory, plugins), `MimeTypeArray`/`PluginArray`, consistent error stack traces, timezone/locale detection.
- **Enhanced wait conditions** — `WaitOptions` with `poll_interval_ms`, `settle_timeout_ms`, `quiet_window_ms` for flexible NetworkIdle detection. `Tab::wait_for_condition_with()` for explicit options.
- **Challenge-aware HTTP client** — network retry/backoff for cleared challenges, interactive/blocked challenge short-circuit.
- **`DomSnapshot::extra_attr()`** — attribute content extraction for metadata parsing.

### Changed

- **Network client** — `HttpClient::request()` returned type updated to `FetchOutcome` with optional `Challenge` field. Interception-aware retry for non-interactive challenges.
- **Session/tab state** — challenge clearance cookie passthrough, structured fetch outcome reporting.
- **Config** — `stealth` flag added to `BrowserConfig` for stealth-mode opt-in.

### Internal

- **JS runtime boot sequence** — `register_window_globals()` replaced with a JS bootstrap script compiled from `V8_PARITY_BOOTSTRAP` template, avoiding native `ObjectInitializer` limitations (no getter/setter support in boa 0.20).
- **wreq 6.0.0-rc** — added as HTTP client dependency alongside `reqwest` for stealth-mode emulation (Chrome JA4+ fingerprint).
- **Code quality** — clippy warnings resolved (collapsible if, `contains_key`, `trim_split_whitespace`); formatting applied.

## [0.15.0] - 2026-06-07

### ⚠️ BREAKING CHANGES

- **MSRV raised from 1.82 to 1.96** — downstream crates must build on Rust 1.96 or later. Pinning an older toolchain against `oxibrowser` `0.15.0` will fail at dependency resolution.
- **Edition upgraded from 2021 to 2024** — the workspace now uses `edition = "2024"`. Transitive consumers in the same workspace will pick this up; downstream crates that depend on `oxibrowser` are unaffected unless they use `cargo metadata` to read our edition.

### Changed

- **Toolchain & edition** — all four crates (`oxibrowser`, `oxibrowser-core`, `oxibrowser-cdp`, `oxibrowser-webapi`) now declare `edition = "2024"` and `rust-version = "1.96"`. CI updated to `dtolnay/rust-toolchain@1.96` (was `@1.82`).
- **Documentation** — README and CONTRIBUTING updated to advertise Rust 1.96+ and Edition 2024.

### Internal

- **Match ergonomics** — `crates/oxibrowser-webapi/src/dom/node.rs:135` `set_text_content` rewritten to drop the explicit `ref mut` binding mode. Edition 2024 disallows explicit borrow modes in implicitly-borrowing patterns.
- **Clippy `collapsible_if` cleanup** — 143 nested-`if` blocks collapsed to let-chains (`if let X = y && condition { ... }`) via `cargo clippy --fix`. Let-chains are stable since Rust 1.88.
- **`assert_matches!` adoption** — 15 occurrences of `assert!(matches!(cmd, ...))` in `crates/oxibrowser/src/session/parser.rs` switched to the stable `std::assert_matches!` macro (Rust 1.96). Better failure diagnostics (prints the actual `Debug` value on mismatch).
- **`Vec::extract_if` adoption** — two retain+count patterns simplified to a single-pass `extract_if(.., predicate).collect()` / `.count()`:
  - `crates/oxibrowser-core/src/js/job_queue.rs::pop_due_timers` (was 8 lines, 2 passes; now 4 lines, 1 pass).
  - `crates/oxibrowser-core/src/browser.rs::cleanup_closed_sessions` (removed length before/after trick; `extract_if().count()` returns removed count directly). Predicate inverted to match: returns `true` for items to remove, `false` for items to keep.
- **Formatting** — `cargo fmt` applied across the workspace; no semantic changes.

## [0.13.0] - 2026-06-04

### ⚠️ BREAKING CHANGES

- **`BrowserEvent` variants now require a `tab_id: Uuid` field.** Every event is from a tab; the field is required at the Rust level. External `match` arms on the variants need to add a binding for `tab_id` (use `tab_id, ..` if you don't care about the value). The wire format is a non-breaking addition — `#[serde(default = "Uuid::nil")]` makes the field optional on deserialize, so older JSON payloads still parse.

### Added

- **`tab_id: Uuid` on every `BrowserEvent` variant** — `NavigationStarted`, `WaitingForSelector`, `DocumentReady`, and `ScreenshotCaptured` now carry the id of the `Tab` that emitted them. Stable for the lifetime of the tab and shared across `Tab::clone`. Exposed via `Tab::tab_id()`. `Browser::new_tab()` generates a fresh `Uuid::new_v4()` per tab.
- **Per-tab event routing** — the foundation for `oxi-agent`'s `OxiBrowserEngine` to route events to the right callback when multiple tabs are open in a single browser. The oxi-agent update follows in a coordinated PR (see `docs/designs/2026-06-04-oxibrowser-observability-followup.md`).
- **2 new unit tests**: `event_tab_id_preserved_in_serde` and `test_tab_id_is_stable_across_clones`.

### Changed

- **Workspace version bumped to `0.13.0`** (all four crates: `oxibrowser`, `oxibrowser-core`, `oxibrowser-cdp`, `oxibrowser-webapi`). The major bump signals the breaking `tab_id` requirement for downstream consumers.
- **Doc comment corrections on `DocumentReady`**:
  - `total_bytes` is the size of the **post-parse, re-serialized HTML body** (`result.html.len() as u64`), **not** the wire-level `Content-Length`. The doc comment previously claimed the latter.
  - `js_script_count` is the count of `<script>` **references** in the DOM's resource list, **not** the count of scripts the JS runtime actually executed. The doc comment now spells out the caveat.
- Workspace `uuid` dependency now enables the `serde` feature so `Uuid` can be a field on `Serialize`/`Deserialize` types.

## [0.12.0] - 2026-06-04

### Added — Browser Observability

- **`BrowserEvent` enum** (`oxibrowser_core::event::BrowserEvent`) — public observability surface for the browser lifecycle. Four variants:
  - `NavigationStarted { url }`
  - `WaitingForSelector { selector, timeout_ms }`
  - `DocumentReady { final_url, title, status, total_bytes, js_script_count, total_duration }`
  - `ScreenshotCaptured { bytes, viewport_width, duration }`
- **`Browser::subscribe_events()`** — returns a `tokio::sync::broadcast::Receiver<BrowserEvent>`. Multiple observers can subscribe; oldest event is dropped on overflow.
- **`BrowserEvent::short_label()`** — single source of truth for user-facing progress text (e.g. `Loaded "Example" — 200 · 1.2 KB · 4 scripts · 245 ms`).
- **Tab events** — `Tab::goto` emits `NavigationStarted` + `DocumentReady`; `Tab::wait_for` emits `WaitingForSelector`; `Tab::screenshot` emits `ScreenshotCaptured`. All emission is non-blocking; events are dropped silently if the observer queue is full.
- **9 new unit tests** in `event.rs` and `browser.rs` (label formatting, wire format, overflow safety, end-to-end subscribe/recv).

### Changed

- `Tab` now holds an optional `broadcast::Sender<BrowserEvent>`. Tabs created via `Browser::new_tab()` are wired to the browser's event stream; tabs built directly via `Tab::new()` (tests) are not.
- Workspace version bumped to `0.12.0` (additive — no breaking changes).

## [0.11.0] - 2026-05-20

### Added — CLI 2.0 (Agent-First Redesign)

- **`fetch`** — universal one-shot command (absorbs `browse` and `eval`)
  - `--format markdown|html|text` (markdown default, human-readable)
  - `--click`, `--fill`, `--press`, `--wait` for interaction
  - `--eval <expr>` for JS evaluation
  - `--summary` for quick page metadata
  - `--fields`, `--max-bytes` for agent-friendly output control
  - `--json` for machine-readable output (opt-in, not automatic)
- **`extract`** — structured data extraction: `--links`, `--title`, `--text`, `--selector`, `--attrs`, `--all`
- **`session`** — stdin/stdout JSON REPL for multi-step automation
  - 22 commands: `new`, `goto`, `back`, `forward`, `reload`, `click`, `fill`, `press`, `type`, `select`, `check`, `uncheck`, `scroll`, `eval`, `extract`, `content`, `screenshot`, `wait`, `close`, `list`, `help`, `exit`
  - Clean shutdown on EOF, `exit`, Ctrl+C, SIGTERM
  - Multi-tab support with tab IDs
- **`run`** — YAML automation scripts with `CliResponse` JSON wrapping
- **`describe`** — CLI schema as JSON (for agent introspection)
- **`skill`** — agent skill guide (markdown or `--json`)
- **`version`** — version info (text or `--json`)
- **Input validation** (`validate.rs`) — URL scheme, control chars, CSS selectors
- **`CliResponse` JSON wrapper** (`output.rs`) — consistent `{ok, data, meta, error, error_code}` format
- **Exit codes**: 0=success, 1=runtime, 2=input validation, 3=timeout, 4=network

### Fixed

- **scroll** — use `scrollTop`/`scrollLeft` instead of `window.scrollBy` (not available in boa_engine)
- **eval quotes** — session parser strips wrapping quotes from JS expressions
- **`--json` consistency** — all commands accept `--json` without error
- **text extraction** — block-level sibling separators for clean line breaks
- **`describe` schema** — added missing `uncheck` command, corrected default format to `markdown`

### Changed

- Default output format is **markdown** (was `html`)
- Human-readable by default; `--json` is opt-in for agents
- Errors are plain text on stderr; JSON with `--json`
- `describe` and `run` always output JSON (no `--json` needed)

## [0.10.0] - 2026-05-18

### Added
- **OXI.getAccessibilityTree** CDP method — semantic tree of page content with roles, labels, visibility, interactivity, and approximate Y positions
- **OXI.getBoxModelScreenshot** CDP method — PNG screenshot with colored boxes representing each DOM element
- **Box Model Renderer** (`css/visual.rs`) — renders elements as colored rectangles with background colors, borders, and text
- **Color parser** — full CSS color support: `#RGB`, `#RRGGBB`, `rgb()`, `rgba()`, `hsl()`, `hsla()`, `currentColor`, and 100+ named colors

### Added (JS API)
- **`getComputedStyle(el)`** — global function and `window.getComputedStyle`, returns CSSStyleDeclaration with computed values
- **`element.getBoundingClientRect()`** — returns DOMRect with x, y, width, height, top, right, bottom, left
- **`element.offsetWidth` / `element.offsetHeight`** — layout-based dimensions
- **`element._visible`** — boolean: `display !== none && visibility !== hidden && opacity !== 0`
- **`element._interactive`** — boolean: not `disabled` + `pointerEvents !== none`
- **`style.getPropertyValue(name)`** — get computed property value

### Added (CSS Layout Engine)
- **`LayoutEngine`** (`css/layout.rs`) — pure-Rust CSS layout approximation:
  - Tag defaults (block, inline, replaced elements)
  - Inline style parsing (`style="color:red"`)
  - CSS inheritance (font, color, visibility)
  - Color/length normalization
  - Width estimation with wrapping
  - Y-position estimation from DOM order
- **`ComputedStyle`** struct — full computed style map with visibility, interactive, colors, dimensions
- **`LayoutRect`** struct — position and size for each element

### Fixed
- Text duplication in accessibility tree (same text no longer shown twice)
- `parse_color_to_rgba` division by zero on scale=0
- Duplicate ID counter bug in test helper
- `take_while` consuming delimiter in test helper parser
- `parent_w - style.margin_top` bug in width estimation
- Clippy warnings throughout codebase

## [0.9.1] - 2026-05-16

### Fixed
- Clippy `doc_lazy_continuation` warning in `runner.rs`
- Cargo fmt formatting issues
- Unused import warnings in `parser.rs`

### CI
- All GitHub Actions CI checks now pass

## [0.9.0] - 2026-05-16

### Added
- **ScriptRunner module**: New `oxibrowser-core/src/script/` module for YAML-based browser automation:
  - `parser.rs`: YAML parsing to `ScriptConfig` (serde_yaml)
  - `runner.rs`: Step-by-step script execution on `Tab` with variable interpolation
  - `types.rs`: Step enum with 30+ step types (navigation, interaction, content, flow control)
  - Supports goto, click, fill, type, wait, evaluate, extract, screenshot, set, echo, sleep, if, retry
- **`oxibrowser run` CLI command**: Run YAML scripts from the CLI (`oxibrowser run <script.yaml>`)
- **Variable interpolation**: `${var}` substitution in step fields, `$$` for literal `$`
- **Error handling**: `on_error.action: abort | continue` with optional screenshot on error

### Changed
- CLI enhanced with `run` subcommand (developer tool only)

### Architecture
- ScriptRunner shared between CLI and future BrowserTool in agent contexts
- No `.programs/oxibrowser` registration needed — agents use BrowserTool directly

## [0.7.0] - 2026-05-16

### Added
- **Mutation persistence**: `createElement`, `createTextNode`, `appendChild`, `removeChild`, `insertBefore`, `setInnerHtml` now apply to webapi DOM — elements survive across `evaluate()` calls and are discoverable via `querySelector`
- **`element.style` as property**: `el.style` is now a CSSStyleDeclaration-like object (not a function) with `getPropertyValue()`, `setProperty()`, `removeProperty()`
- **`element.classList` as property**: `el.classList` is now a DOMTokenList-like object (not a function) with `add()`, `remove()`, `toggle()`, `contains()`
- **`element.textContent` setter**: Read/write — `el.textContent = 'new'` updates live snapshot + records mutation for webapi DOM
- **`element.innerHTML` setter**: Read/write — `el.innerHTML = 'html'` updates live snapshot + records mutation
- **`data-oxi-text` bridge**: Snapshot regeneration reads `data-oxi-text` attribute as fallback for text content set via JS
- **14 new DOM APIs** in `create_element_object()`:
  - Tree traversal (accessors): `firstChild`, `lastChild`, `nextSibling`, `previousSibling`
  - Tree manipulation (methods): `insertBefore`, `replaceChild`, `removeAttribute`, `cloneNode`, `remove()`
  - Style/Class (methods): `style()`, `classList()`
  - Focus/Form (noop): `focus()`, `blur()`, `submit()`

### Fixed
- **`getAttribute` / `hasAttribute`**: Was reading from static cloned HashMap, now reads from live `DomSnapshot` via `Arc<RwLock>`
- **`input.value` getter**: Was capturing initial value at creation, now reads from live snapshot
- **`input.value` setter**: Was only recording mutation, now also updates snapshot attribute immediately
- **`click()`**: Was only recording mutation, now also fires registered JS event handlers from `__listeners`
- **`createElement`**: Was returning minimal stub, now calls `create_element_object()` for full element with all APIs
- **110 code quality issues**: Security, data integrity, API completeness, CSS rendering, testing, dependency hygiene

### Changed
- `apply_mutations()` now applies all 5 structural mutations (CreateElement, CreateTextNode, AppendChild, RemoveChild, SetInnerHtml) to webapi DOM
- `Frame::document_mut()` bumps `dom_version` counter on each mutation
- webapi `Document` gains `create_element_node()`, `create_text_node()`, `tree_mut()`, `nodes_mut()`

### Tests
- 279 tests pass (223 core + 23 E2E + 20 webapi + 10 event + 3 smoke)
- 22/22 scenario tests pass (real websites: httpbin, Hacker News)

## [0.6.0] - 2026-05-14

### Added
- **Input domain**: `Input.dispatchKeyEvent`, `Input.dispatchMouseEvent`, `Input.insertText` — dispatch real `KeyboardEvent`/`MouseEvent` via JS evaluation on `document.activeElement` / `document.elementFromPoint()`
- **document.activeElement**: JS getter returning `document.body` (no real focus tracking)
- **document.elementFromPoint(x, y)**: JS method approximating element hit-testing by DOM order with estimated element heights
- **Page.captureScreenshot**: Real PNG output using built-in 8×16 bitmap font (ASCII 32–126) — renders DOM text content as white-background image, base64-encoded PNG response
- **Fetch domain complete**: `continueRequest` (modify headers/URL/postData, resume), `failRequest` (fail with error reason), `fulfillRequest` (synthetic response), `getResponseBody` — with `PausedRequestRegistry` for request tracking
- **HttpClient.intercept()**: HTTP fetch with `InterceptAction` (Continue/Fail/Fulfill) — enables Fetch domain interception integration
- **InterceptedResponse**: New error variant for synthetic HTTP responses from Fetch.fulfillRequest
- **Input JS helpers**: `js_dispatch_key_event()`, `js_dispatch_mouse_event()`, `js_insert_text()` — generate JS code strings for Input domain dispatch

### Tests
- 208 tests pass (164 core + 23 E2E + 18 webapi + 3 smoke)

## [0.5.0] - 2026-05-13

### Added
- **CSS text screenshot**: `page.to_text_screenshot()` — ASCII/Unicode DOM rendering with block element tags, indentation, BR/HR/IMG handling
- **document.write()**: Appends HTML content as text node to body
- **MutationObserver**: Constructor with `observe()`, `disconnect()`, `takeRecords()` stubs
- **Puppeteer smoke tests**: 3 E2E tests verifying Puppeteer/Playwright CDP compatibility (built-in HTTP server, WebSocket client, process spawning)

### Tests
- 205 tests pass (152 core + 3 smoke + 22 E2E + 18 webapi + 10 event)

## [0.4.0] - 2026-05-13

### Added
- **DOM Mutation**: `document.createElement(tag)`, `document.createTextNode(text)`, `element.appendChild(child)`, `element.removeChild(child)` — full DOM mutation with `DomSnapshot` sync
- **fetch Response**: `.text()` → `Promise<string>`, `.json()` → `Promise<object>`, `headers` object, `bodyUsed`, `type` properties
- **XMLHttpRequest**: Constructor with `.open()`, `.send()`, `.setRequestHeader()`, `.getResponseHeader()`, `.abort()`, `onload`/`onerror`/`onreadystatechange` callbacks
- **Real-world integration tests**: `createElement` on real page, window globals verification

### Changed
- **Clippy**: Zero warnings across entire workspace (was 48+)
- **fetch Response**: Body serialization via `serde_json` (no more string injection bugs)

### Tests
- 201 tests pass (151 core + 22 E2E + 18 webapi + 10 event)

## [0.3.0] - 2026-05-13

### Added
- **Logs to stderr**: `tracing_subscriber` now writes to stderr, stdout is clean data output
- **window global**: `navigator`, `location`, `performance`, `viewport`, `crypto` properties
- **document.body/head/documentElement**: Real DOM elements as JS objects
- **Real fetch()**: Channel-based JS↔HttpClient bridge with `FetchRequestMsg`/`FetchResponseMsg`
- **localStorage**: Full Storage interface (getItem/setItem/removeItem/clear/key/length)
- **atob/btoa**: Base64 encode/decode via `base64` crate
- **URLSearchParams**: Constructor with get/set/append/delete/has/forEach/toString
- **URL class**: Constructor with `url::Url` parsing and accessor getters
- **crypto.getRandomValues**: Pseudo-random byte generation
- **TextEncoder/TextDecoder**: UTF-8 encode/decode
- **EventTarget**: Real `addEventListener`/`removeEventListener`/`dispatchEvent` on document and elements
- **CDP Network cookies**: `getAllCookies`, `getCookies`, `setCookie`, `deleteCookies` with full CRUD
- **CDP Fetch domain**: Full implementation with event interception pattern

### Tests
- 194→201 tests

## [0.2.0] - 2026-05-13

### Added
- **CI/CD**: GitHub Actions workflow (check, test, clippy, fmt, release build)
- **Container**: Dockerfile with multi-stage build for minimal image size
- **Security**: CDP server connection limit (max 16 concurrent)
- **Security**: CDP message size validation (max 1MB)
- **Benchmarks**: Performance benchmarks for HTML parsing, DOM queries, Markdown conversion
- **CHANGELOG.md**: This file

### Changed
- **Runtime**: Replaced `std::sync::RwLock` with `parking_lot::RwLock` in JS runtime (poison-free)
- **Safety**: Eliminated all production `unwrap()` calls — replaced with `expect()`, safe patterns, or proper error propagation
- **Safety**: `as_object().unwrap()` replaced with safe `if-let` pattern in JSON serialization

### Fixed
- Potential runtime panics from poisonable `std::sync::RwLock` in JS runtime
- Potential runtime panics from `unwrap()` on `Option` and `Result` types in production code

## [0.1.0] - 2026-05-12

### Added
- **Browser lifecycle**: `Browser`, `Session`, `Page`, `Frame` hierarchy with thread-safe IDs
- **HTML parsing**: html5ever-based DOM parsing with CSS selectors
- **JS Runtime**: boa_engine integration for real JavaScript execution (ES2024+)
  - Persistent context across `evaluate()` calls
  - `console.log/warn/error/info` support
  - `document.querySelector`, `document.querySelectorAll`, `document.title`
  - DOM mutation tracking (click, setAttribute, input value)
  - Runtime limits (loop iteration, recursion, stack size, timeout)
- **CDP Server**: Chrome DevTools Protocol over WebSocket
  - HTTP endpoints: `/json/version`, `/json`
  - WebSocket upgrade with RFC 6455 compliance
  - 7 domain handlers: Browser, DOM, Fetch, Network, Page, Runtime, Target
  - Event broadcasting: frameNavigated, domContentLoadedEventFired, loadEventFired
  - Network events: requestWillBeSent, responseReceived, loadingFinished
  - Runtime events: executionContextCreated, consoleAPICalled
- **Network**: reqwest-based HTTP client with cookie injection
- **CookieJar**: Domain-scoped cookie storage
- **Document**: CSS selectors, text extraction, Markdown conversion, resource URL extraction
- **Tree**: Adjacency list with DFS/BFS traversal
- **CLI**: `fetch`, `serve`, `version` subcommands via clap
- **Tests**: 185 tests (142 core + 15 E2E + 18 webapi + 10 integration)
- **Encoding**: charset detection and encoding conversion via encoding_rs

[0.7.0]: https://github.com/a7garden/oxibrowser/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/a7garden/oxibrowser/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/a7garden/oxibrowser/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/a7garden/oxibrowser/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/a7garden/oxibrowser/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/a7garden/oxibrowser/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/a7garden/oxibrowser/releases/tag/v0.1.0
