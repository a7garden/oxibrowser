# Session Handoff — 2026-08-11

> **For the next session:** Read this document first. It summarizes what was done,
> what was designed, and what you should implement next.

## Session Summary

This session performed three categories of work on OxiBrowser (v0.20.0):
1. **Immediate fixes** — Input domain dead-code bug, README staleness, binary size verification
2. **WASM design** — Full design spec + TDD implementation plan for WebAssembly support
3. **Documentation** — All changes committed to `main`

All 3 commits are on `main`, working tree is clean:
```
ca11cd8 docs: WASM support design spec + implementation plan
2ea12dd docs: update README stale numbers + add strip release profile
15b6305 fix(cdp): route Input domain in dispatch — was dead code
```

---

## What Was Done

### 1. Input Domain Routing Fix (commit 15b6305)

**Bug:** `input.rs::handle` (10 CDP methods: `dispatchKeyEvent`, `dispatchMouseEvent`, `dispatchDragEvent`, `insertText`, `imeSetComposition`, `synthesizePinchGesture`, `synthesizeScrollGesture`) existed but `mod.rs::dispatch` had no `"Input"` match arm. All `Input.*` CDP methods returned `"unknown domain: Input"`. This meant Puppeteer/Playwright `page.mouse()` / `page.keyboard()` were broken over CDP. CLI `session` click/fill/press worked because they bypass CDP and use JS evaluation directly.

**Fix:** Added `"Input" => input::handle(method_name, params, ctx).await,` to the dispatch match in `crates/oxibrowser-cdp/src/domains/mod.rs`.

**Verified:** `cargo build --release -p oxibrowser-cdp` passes; 24 CDP tests pass.

### 2. README Overhaul + Strip Profile (commit 2ea12dd)

**Problem:** README was stuck at v0.17.0 — 3 versions behind. Stats were wrong (24 MB binary, 554 tests, 10 domains, 30K lines). The "What's New in 0.17.0" section required manual updates every release and inevitably goes stale.

**Fixes applied:**
- Removed "What's New in 0.17.0" section entirely; replaced with a one-line CHANGELOG link + high-level milestone summary. **Pattern going forward: version-specific content goes in CHANGELOG.md, never in README.**
- Updated all stats to actual values: **~44 MB binary** (was 24 MB before rendering deps), **697 tests** (was 554), **12 CDP domains** (was 10), **~47,500 lines** (was 30,206).
- Updated architecture diagram, crate structure table (added `oxibrowser-render`, removed `oxibrowser-webapi` which was merged into core), CDP domain table (added Emulation/Log/Tracing/Input methods), network layer table (added CORS/auth/proxy/stealth/WebSocket), and replaced "CSS Text Rendering" with "Rendering Pipeline" describing the Blitz/Stylo/Taffy/Parley stack.
- Added `[profile.release] strip = true` to workspace `Cargo.toml`.

### 3. Binary Size Measurement

**Finding:** README's "24 MB" was pre-rendering-deps. With Blitz/Stylo/Taffy/Parley:
- **Unstripped:** 51 MB
- **Stripped (manual `strip`):** 43 MB
- **Stripped (`strip=true` profile, clean build):** 44 MB

**Note:** `strip = true` in `[profile.release]` requires a **clean build** (`cargo clean` then rebuild) to take effect — incremental builds don't re-strip the final binary. Verified this behavior during the session. The next release build (CI or `cargo clean && cargo build --release`) will produce the stripped binary.

### 4. WASM Design + Implementation Plan (commit ca11cd8)

Two documents written:
- **Spec:** `docs/superpowers/specs/2026-08-11-wasm-support-design.md` — Architecture decision (wasmi over wasmtime), threading model, value bridge, API surface, file structure, scope boundaries, risks, open questions
- **Plan:** `docs/superpowers/plans/2026-08-11-wasm-support.md` — 7-task TDD plan with exact WASM byte arrays, code scaffolding, and verification steps

---

## What the Next Session Should Do

### Primary: Implement WASM Support

Execute the implementation plan at:
```
docs/superpowers/plans/2026-08-11-wasm-support.md
```

Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` skill.

**Quick-start context for WASM implementation:**
- **Runtime:** `wasmi` v1.x (pure Rust WASM interpreter, Apache-2.0)
- **File to create:** `crates/oxibrowser-core/src/js/wasm.rs` (~700-900 lines)
- **Key integration point:** `register_wasm_globals()` called from `create_context()` in `runtime.rs`
- **Threading:** Everything synchronous on the JS thread (boa `Context` is `!Send`)
- **Host-function bridge:** Thread-local raw pointer to `Context` (same pattern as existing `LISTENER_REGISTRY`)
- **Store management:** `Rc<RefCell<Store<u32>>>` for shared mutable access

**Open questions to resolve during implementation:**
1. wasmi exact latest version on crates.io (spec says 0.40+; verify)
2. boa 0.20 `BigInt` support for `i64` WASM values
3. wasmi `externref` support in v1.x
4. Fuel metering (spec recommends default 10M instruction limit)

### Secondary: Known Issues Not Yet Addressed

These were identified during the capability audit but not fixed in this session:

1. **`window.parent` / `window.top` (W3c)** — Not yet implemented. Nested-iframe contexts exist but cross-frame references don't resolve.
2. **Dynamic iframe creation (W3d)** — `document.createElement('iframe')` + append doesn't create a new execution context.
3. **Child-target CoreEvent drainer** — Multi-tab child targets (created via `Target.createTarget`) don't drain their own CoreEvents. Load events etc. for child tabs may not fire.
4. **`getComputedStyle` from LayoutEngine** — Only inspects inline `style=` attributes; applied CSS rules (from `<link>`/`<style>`) show up in `Page.captureScreenshot` (Blitz renders them) but not in JS `getComputedStyle()`.
5. **Shadow-aware screenshot is lossy** — CSSOM inline styles, listeners, and stylesheet computed styles are not in the DomSnapshot used for rasterization. Structural + `style=` fidelity is preserved.

---

## Current OxiBrowser Capability Snapshot (v0.20.0 + Input fix)

| Dimension | Status |
|-----------|--------|
| **Binary** | ~44 MB stripped, ~8 MB base RAM, ~50 ms cold start |
| **CDP** | 12 domains, ~104 methods (all routed now), Puppeteer/Playwright compatible |
| **JS Runtime** | boa_engine ES2024+, 19,000 lines of Web API bridge |
| **Rendering** | Blitz + Stylo + Taffy + Parley (PNG, PDF, @font-face, shadow-DOM-aware) |
| **DOM** | Full manipulation, Shadow DOM (open/closed/declarative/slots), custom elements |
| **Events** | Bubbling, constructors, dispatchEvent, rAF |
| **Network** | Async fetch/XHR/WebSocket, CORS+preflight, cookies (PSL/prefixes/CHIPS), proxy, auth, stealth |
| **Multi-tab** | ✅ Target.createTarget creates real sessions |
| **Iframe** | Per-frame JS execution contexts (srcdoc, about:blank, nested) |
| **WASM** | ❌ **Not yet — this is the next task** |
| **Tests** | 697 test functions + 8/8 acceptance harness |
| **Codebase** | ~47,500 lines across 4 crates |
