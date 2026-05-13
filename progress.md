# OxiBrowser Progress

## 2026-05-14 — Deep Code Analysis Complete

### Status: Analysis Written
- **Output:** `/tmp/analysis/oxibrowser-deep.md` (43.5 KB)
- **Scope:** Full workspace analysis — 4 crates, 38 source files, ~10,621 LOC

### Key Findings

#### Build Status: ⚠️ Broken
- **Compile error** in `crates/oxibrowser-core/src/js/runtime.rs` test `test_js_runtime_config_custom` — missing `viewport_width`/`viewport_height` fields in `JsRuntimeConfig`
- **2 warnings:** unused variables `window_obj` (line 1493) and `snapshot` (line 1599)

#### Architecture Summary
- 4 crates: `oxibrowser-webapi` (leaf) → `oxibrowser-core` → `oxibrowser-cdp` → `oxibrowser` (binary)
- Browser → Session → Page → Frame hierarchy (Lightpanda-inspired)
- boa_engine JS runtime on dedicated OS thread with persistent Context
- Full CDP WebSocket server (7 domains, 15 E2E tests)
- html5ever for HTML parsing, reqwest for HTTP, encoding_rs for charset detection

#### Test Coverage
- ~149 tests total (unit + integration)
- 15 E2E tests using raw TCP test server + tokio-tungstenite client
- Criterion benchmarks for HTML parsing, DOM queries, markdown conversion
- Tests cannot currently run due to compile error

#### Major Stubs
- `fetch()` JS API — returns hardcoded 200 response
- `setTimeout`/`setInterval` — synchronous execution (no event loop)
- Screenshot — 1x1 transparent PNG placeholder
- PDF export — empty data
- DOM mutation — no remove/insert/reparent operations
- Fetch interception — notification-only, doesn't pause requests

### Files Analyzed
All 38 source files across the workspace, plus Cargo.toml files and benchmarks.
