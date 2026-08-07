# OxiBrowser → Headless-Chrome + Playwright Parity Roadmap

> **Status:** Design / living roadmap. Authored 2026-08-07.
> **Hard constraint:** Stay **pure Rust** — no Chromium, no V8. `boa_engine` (JS),
> `html5ever` (HTML), Blitz stack (Stylo + Taffy + Parley + vello) for layout/render,
> `wreq`/`btls` for network.
> **Target:** Everything a *headless Chrome* session driven by *Playwright* can do —
> navigate, run the page's JS, interact (click/type/wait), screenshot, evaluate —
> OxiBrowser must also do, end to end.

---

## 1. Where we are today (evidence-backed gap summary)

| Area | State | Decisive evidence |
|---|---|---|
| **Page scripts on navigation** | ❌ Never run | `navigate()` (session.rs:345) → `inject_dom_snapshot()` → `set_document()`; `SetDocument` handler (runtime.rs:982) builds `RenderDocument` and returns — **zero `<script>` execution** |
| JS Web API breadth | 🟡 core only | DOM/fetch/timer/Promise/event injected (runtime.rs:1311–7542); **matchMedia, WebSocket, FormData, canvas2D, AbortController, Shadow-DOM render = 0 hits** |
| Event loop | ❌ none | timers/microtasks drain only at end of `evaluate()` (runtime.rs:817,1175); no idle tick |
| JS fetch concurrency | ❌ blocking | `fetch`/`XHR` block JS thread on `recv()` (runtime.rs:1510, TODO #async-fetch) |
| CSS layout / render | ✅ strongest | Blitz path (Stylo+Taffy+Parley+vello_cpu) produces **real pixel PNG** (paint.rs:20). Dual DOM + unmanaged fonts remain |
| CDP protocol | 🟡 partial | 10 domains; **no Emulation/Log**, `Target.create/attach` stubs, events carry no `sessionId`, `exceptionThrown` absent |
| Network correctness | 🟡 transport ok | HTTP/1.1+H2+TLS+gzip/br ✅; **CORS/preflight/proxy/auth/Referer = 0**, cookie expiry/PSL/CHIPS missing |
| Interaction / hit-test | 🟡 fragile | click via approximate `elementFromPoint` (runtime.rs:~4748); `focus/blur/scrollIntoView` no-ops; `wait_for` polls **static** snapshot |
| iframes | ❌ static | `Frame` holds snapshot only; nav never populates children |

**Single biggest blocker:** page JavaScript does not execute on navigation. That alone
blocks ~80% of real Playwright flows. Everything else is secondary until this is fixed.

---

## 2. Design principle: the keystone is the live page

A real browser is a **feedback loop**: HTML parse → script run → mutate DOM → timers/network
fire → re-render → events → more script. OxiBrowser currently has the loop *open*: it parses,
renders a static snapshot, and runs JS only when an agent manually `evaluate()`s.

Closing the loop is the thread that runs through every phase below. We sequence work by
**leverage on that loop**, not by feature popularity.

```
            ┌──────── navigate: fetch HTML ───────┐
            ▼                                       │
   parse → RenderDocument ──► run <script> ──► DOMContentLoaded/load
            │                      │                  │
            │                      ▼                  ▼
            │            timers / fetch / Promise pump (event loop)
            │                      │
            ▼                      ▼
        Blitz render ◄──── live RenderDocument (single source of truth)
            │
            ▼
   screenshot / hit-test / CDP events
```

---

## 3. Phased roadmap

Each phase = own spec → plan → TDD implement → verify → commit. Phases are ordered so
each one *unblocks the most real-world automation that the previous phases did not*.

### Phase 1 — Script execution on navigation (KEYSTONE)  *(this session)*
Make the page's `<script>` tags execute, in order, after the document is built; fire
`DOMContentLoaded`/`load`; pump timers/microtasks to an idle settle point. Unlocks every
SPA whose bootstrap is "load bundle → render into `#app`".

### Phase 2 — Real event loop
Timers/microtasks tick independently of `evaluate()`; `network-idle` detection; `wait_for_*`
observes the **live** DOM. So `setTimeout`-driven flows, debounced search, lazy hydration,
and Playwright auto-waiting all work.

### Phase 3 — Async (non-blocking) fetch / XHR
`fetch`/`XHR` return pending Promises; concurrent in-flight requests; resolve on the event
loop. Removes the serialization that cripples SPA bootstrap latency and parallel API loads.

### Phase 4 — Missing Web APIs (by SPA impact)
Priority order: `matchMedia` (dark-mode/responsive/CSS-in-JS), `WebSocket`, `FormData` +
file upload, `canvas` 2D, `AbortController`/`Signal`, real Shadow DOM + custom-element
lifecycle, `URL.createObjectURL`/blob, `Element.matches/closest`.

### Phase 5 — CDP completeness (Playwright compatibility)
`Emulation.setDeviceMetricsOverride`, `Log.enable`, flat-protocol `sessionId` multiplexing,
real `Target.create/attach` (multi-target), `Runtime.exceptionThrown` +
`consoleAPICalled` as live events, missing `DOM.*` methods, `Page.handleJavaScriptDialog`.

### Phase 6 — Network correctness
CORS + preflight (Origin, `Access-Control-*`), cookie expiry/Max-Age, Public Suffix List,
CHIPS partitioned + `__Host-`/`__Secure-`, proxy (HTTP/SOCKS), auth (basic/digest),
automatic `Referer`, streaming bodies.

### Phase 7 — Render & interaction fidelity
Unify the JS DOM and Blitz `RenderDocument` into one live tree (drop the serialize→reparse
screenshot path); wire `fontdb` font loading + `@font-face`; **layout-based hit-testing**
(`elementFromPoint` from Taffy layout boxes, not estimation); `printToPDF`.

### Phase 8 — iframes & multi-frame
Populate child frames on navigation; execute frame scripts in isolated contexts; hit-test
and evaluate across frame boundaries.

### Phase 9 — Long-tail Playwright surface
`alert/confirm/prompt` dialogs, downloads, multi-tab (`context.newPage()`), geolocation/
timezone/viewport emulation, request interception ergonomics, tracing.

---

## 4. Sequencing rationale (why this order)

1. **Phase 1 before all** — without running scripts, no SPA works, so every other phase is
   untestable against real sites.
2. **Phase 2 before 3** — a pumpable event loop is the *prerequisite* for async fetch to
   resolve promises; blocking-but-pumped (Phase 1+2) already makes serial SPA bootstraps
   work, so 3 is latency/parallelism, not correctness.
3. **Phase 4 after the loop exists** — new APIs are only useful once scripts run and the loop
   pumps. `matchMedia` is prioritized because it throws in a huge share of production SPAs.
4. **Phase 5 interleaves** — CDP completeness is what lets Playwright *drive* the engine;
   it grows in lockstep with each capability phase (Phase 1 must emit `Page.lifecycleEvent`
   / `Runtime.executionContextCreated`).
5. **Phase 7 (render/hit-test) is high-value but not blocking** the loop; it raises fidelity
   for screenshot/assert parity.
6. **Phases 8–9** are breadth for the long tail; schedule after the core loop is trustworthy.

---

## 5. Non-goals (explicit, to prevent scope creep)

- **Not** pixel-perfect Chrome rendering parity (Blitz is close; chasing sub-pixel identity
  with Skia/Blink is a multi-year sink). "Visually correct for automation" is the bar.
- **Not** replacing `boa_engine` with V8 or adopting any C/C++ JS engine. Pure-Rust only.
- **Not** solving anti-bot challenges (Cloudflare Turnstile etc.) — detection + retry only.
- **Not** a full DevTools frontend. CDP *server* parity for Playwright/Puppeteer only.

---

## 6. Verification bar per phase

Every phase ships with:
1. A unit/integration test that **fails on `main`** and passes after the phase.
2. A real-site or mock-server smoke test exercising the changed path end to end.
3. `cargo build --workspace` + `cargo test --workspace` green.
4. One commit per task, conventional-commit messages.

The ultimate acceptance test, revisited each phase: **a Playwright script that navigates to
a React SPA, fills a login form, clicks submit, waits for a dashboard, and screenshots —
succeeds against `oxibrowser serve`.** We tag the repo when this first passes.
