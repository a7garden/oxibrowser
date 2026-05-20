# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
