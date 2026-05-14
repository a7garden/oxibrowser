# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.0] - 2025-05-14

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

## [0.5.0] - 2025-05-13

### Added
- **CSS text screenshot**: `page.to_text_screenshot()` — ASCII/Unicode DOM rendering with block element tags, indentation, BR/HR/IMG handling
- **document.write()**: Appends HTML content as text node to body
- **MutationObserver**: Constructor with `observe()`, `disconnect()`, `takeRecords()` stubs
- **Puppeteer smoke tests**: 3 E2E tests verifying Puppeteer/Playwright CDP compatibility (built-in HTTP server, WebSocket client, process spawning)

### Tests
- 205 tests pass (152 core + 3 smoke + 22 E2E + 18 webapi + 10 event)

## [0.4.0] - 2025-05-13

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

## [0.3.0] - 2025-05-13

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

## [0.2.0] - 2025-05-13

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

## [0.1.0] - 2025-05-12

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

[0.2.0]: https://github.com/oxibrowser/oxibrowser/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/oxibrowser/oxibrowser/releases/tag/v0.1.0[0.6.0]: https://github.com/a7garden/oxibrowser/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/a7garden/oxibrowser/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/a7garden/oxibrowser/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/a7garden/oxibrowser/compare/v0.2.0...v0.3.0
