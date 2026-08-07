# Phase 1 Spec — Page Script Execution on Navigation

> Roadmap: `docs/superpowers/specs/2026-08-07-chrome-parity-roadmap.md` (Phase 1, keystone).
> Hard constraint: pure Rust (`boa_engine` + Blitz `RenderDocument`). No new C deps.

## Goal

When `Session::navigate(url)` completes, the page's `<script>` tags have executed in
document order, `DOMContentLoaded` and `load` have fired, and the timer/microtask queue has
settled to idle. A subsequent `evaluate("...")` or DOM query observes the **post-script**
state — exactly as it would in headless Chrome.

## Problem (evidence)

- `navigate()` (session.rs:345) calls `inject_dom_snapshot()` → `JsRuntime::set_document()`.
- The `SetDocument` JS-thread handler (runtime.rs:982) builds a `RenderDocument` via
  `from_html` and returns `Done`. **No `<script>` is ever evaluated.**
- `load_sub_resources()` (session.rs:953) fetches `<script src>` as **text** and stores it;
  it is called only from `Tab` (tab.rs:802), never during navigation, and never executed.

## Design

### D1 — Script extraction (async side, from the parsed `DomSnapshot`)

The `Frame` already holds a parsed `DomSnapshot`; `DomNode` has `tag`, `attributes`,
`text_content`, `children` (dom_snapshot.rs:55). Add an extractor that walks the tree in
**document order** and emits, per `<script>` element:

```rust
pub struct ScriptSource {
    pub source: String,        // inline text OR fetched external body
    pub src_url: Option<String>, // Some for external, None for inline
    pub kind: ScriptKind,        // Classic | Module
    pub execute: ExecuteTiming,  // Defer(=document-order-after-parse) | Async
}
```

Semantics for Phase 1 (covers the dominant SPA pattern; parser-blocking + `document.write`
are documented non-goals):

| Attribute | Phase 1 behavior |
|---|---|
| inline, no `src` | execute in document order after full DOM build |
| `<script src>` (classic) | fetch async, execute in document order |
| `defer` or `type="module"` | same bucket as above (ordered, post-parse) |
| `async` | treated as ordered-post-parse for now (unordered timing rarely load-bearing) |
| `type` other than js/module | skip |
| external fetch fails | log + continue (mirrors browser onerror; no throw) |

### D2 — External script fetch during navigation

In `navigate()`, after `Page::from_html`, gather `ScriptSource`s from the active page's root
frame. For entries with `src_url`, resolve against the page URL and fetch via the existing
`http_client.fetch_text()` (same path `load_sub_resources` uses), filling `source`. Collect
into one ordered `Vec<ScriptSource>` and pass it through to the JS thread.

Fetch is **sequential and in-order** for Phase 1 (correct ordering, simplest). Parallel
fetch is Phase 3 (async fetch). This is acceptable because Phase 1 fetch still blocks the
async task, not the JS thread.

### D3 — Extend `JsCommand::SetDocument` to execute scripts

Add a field to the existing command (backward compatible — empty vec = current behavior):

```rust
SetDocument {
    html: String,
    base_url: Option<String>,
    viewport: (u32, u32),
    scripts: Vec<ScriptSource>,   // NEW
}
```

JS-thread handler, after `RenderDocument::from_html` succeeds:

1. Set `document.readyState = "interactive"`.
2. For each `ScriptSource` in order:
   - `ctx.eval(Source::from_bytes(&script.source))`.
   - On `Err`: format via `format_js_error`, `tracing::warn!`, **continue** (a thrown
     script does not abort siblings — matches Chrome). Stash the last exception for CDP
     `Runtime.exceptionThrown` (Phase 5 wiring; Phase 1 just logs).
   - After each script: `ctx.run_jobs()` (microtasks) + `drain_timers()`.
3. Fire `DOMContentLoaded` on `document`; set `readyState = "complete"`; fire `load` on
   `window`.
4. **Bootstrap pump** (D5) to settle pending timers/microtasks.

Limits — **dedicated to the nav-script path, NOT `evaluate()`'s caps.** The existing
`evaluate()` defaults (`max_loop_iterations: 100_000`, `max_recursion: 100`, `max_stack_size:
1024`, `timeout_ms: 5_000`, runtime.rs:348–351) are tuned for agent one-shot snippets. A real
SPA bundle runs **millions to tens of millions** of loop iterations just to init (store setup,
babel/runtime helpers, list rendering, regex), and a JIT-less engine like boa runs *more*
bytecode-loop iterations for equivalent work. Reusing the 100k cap would make "logged and
skipped" the **default** path on real sites — `navigate` "succeeds" but zero scripts ran, a
silent no-op. So the nav-script path gets its own limit set:

- `nav_script_max_loop_iterations: 500_000_000` (5_000× the eval cap; high enough that no
  legitimate bundle trips it, low enough that a pure `while(true){}` is still bounded).
- `nav_script_max_recursion: 4_096`, `nav_script_max_stack_size: 16_384` (protect against real
  stack-overflow *panics* while allowing framework-scale call depth).
- `nav_script_timeout_ms: 30_000` wall-clock budget covering **all** scripts + the settle pump
  cumulatively. **The timeout is the primary guard**; the loop counter is secondary.
- On timeout exhaustion: **stop** running further scripts, `tracing::warn!`, but **do not reset
  the context** (unlike `evaluate()`'s reset-on-timeout at runtime.rs:826). A mid-bootstrap
  context reset would discard the partial DOM/globals every other script built; keeping
  partial state matches Chrome's "slow script" tolerance and avoids a catastrophic cliff.
  A script that *throws* still does not abort siblings (continue).

These four fields are added to `JsRuntimeConfig` and mirrored on `BrowserConfig` with the
defaults above; `From<&BrowserConfig>` propagates them.

### D4 — `readyState` + lifecycle events

`document.readyState` is currently a static bootstrap value. Make it backed by a thread-local
`Cell<&'static str>` (or a JS global `__oxiReadyState`) that the `document.readyState` getter
reads. The handler updates it at each transition. Lifecycle events are dispatched through the
**existing** JS event path: after scripts run, the handler evaluates
`document.dispatchEvent(new Event('DOMContentLoaded'))` then
`window.dispatchEvent(new Event('load'))`. This reuses the listener registry
(runtime.rs:53) — `addEventListener('DOMContentLoaded', cb)` during a script registers on the
document node, and the dispatch fires it.

### D5 — Bootstrap pump (settle to idle)

After `load`, loop (sharing the `nav_script_timeout_ms` wall-clock budget from D3 — the pump
and script execution are one timed phase — plus a 200-pass safety bound):

```
loop {
    let fired = run_jobs_once();        // microtasks
    drain_timers();                      // due timers + their microtasks
    if !fired_anything_this_pass { break }
}
```

Because Phase 1 fetch is blocking, any fetch a script makes resolves **before** the script
returns, so the pump mainly settles `setTimeout` chains. (Phase 2 makes the pump the primary
idle driver.)

### D6 — `inject_dom_snapshot` wiring

`inject_dom_snapshot()` (session.rs:746) currently builds `html`+`url` and calls
`set_document(html, url, viewport)`. Extend it to also extract scripts from the active page
and pass them. `navigate()` already calls `inject_dom_snapshot()`, so no change to `navigate`
itself is needed beyond the script-gathering step inside `inject_dom_snapshot` — except that
external script fetches need the `http_client` + `in_flight`, which the `Session` owns.

## Interfaces (locked)

- `dom_snapshot::extract_scripts(&self) -> Vec<ScriptSource>` — new method on `DomSnapshot`
  (and re-exported via `Frame::extract_scripts`).
- `pub struct ScriptSource { source, src_url, kind, execute }` + `ScriptKind`, `ExecuteTiming`
  in `crates/oxibrowser-core/src/js/dom_snapshot.rs`.
- `JsRuntimeConfig` gains `nav_script_max_loop_iterations`, `nav_script_max_recursion`,
  `nav_script_max_stack_size`, `nav_script_timeout_ms` (defaults: 500_000_000 / 4_096 /
  16_384 / 30_000), mirrored on `BrowserConfig` and propagated by `From<&BrowserConfig>`.

## Tests (acceptance)

1. **Unit (dom_snapshot):** a DomSnapshot with inline + external + defer + module + non-js
   scripts extracts the right `Vec<ScriptSource>` in document order with correct flags.
2. **Unit (JsRuntime):** `set_document` with an inline script that sets `window.__t = 1`;
   after settle, `evaluate("window.__t")` → `1`.
3. **Unit (JsRuntime):** two ordered scripts, second reads a global the first set → ordered.
4. **Unit (JsRuntime):** a script that throws does not prevent a later script from running.
5. **Unit (JsRuntime):** `document.addEventListener('DOMContentLoaded', …)` callback ran
   (observable side effect) after `set_document` returns.
6. **Unit (JsRuntime):** `document.readyState` is `"complete"` after `set_document`.
7. **Integration (Session + mock HTTP server):** navigate to a page with an **external**
   `<script src="/app.js">` (served by the mock) that writes into `#app`; after navigate,
   `evaluate("document.getElementById('app').textContent")` → the rendered value.
8. **Integration:** a script using `setTimeout(() => …, 50)` that sets a flag — after settle
   the flag is set (pump works).
9. **Unit (JsRuntime, limit magnitude):** a synthetic heavy script — a `for` loop of
   **5_000_000** iterations that increments a counter and writes the final value into a DOM
   element's `textContent` — runs to completion under the nav-script limits (this loop is
   50× the default `evaluate()` cap of 100k, so it fails the "reuse eval limits" design and
   proves the dedicated limits). Assert the element reads the expected count and the script was
   **not** skipped.

## Non-goals (Phase 1)

- Parser-blocking script semantics / `document.write`.
- Async (non-blocking) fetch — scripts' `fetch()` still blocks the JS thread (Phase 3).
- `async` script unordered timing fidelity (treated as ordered).
- Cross-origin/CORS enforcement on external script fetch (Phase 6).
- Multi-frame script execution (Phase 8).
- `Runtime.exceptionThrown` CDP emission (Phase 5; Phase 1 only logs).
