# OxiBrowser Chrome-Parity — Phase 4 Progress & Next-Session Handoff

**Date:** 2026-08-08
**Branch:** `main` (20 commits ahead of `origin/main`, **not pushed**)
**Working tree:** clean
**HEAD:** `e8515c1 feat(core): AbortController/AbortSignal (Phase 4)`

---

## What shipped this session (Phase 4)

### WebSocket (full standard) — the headline
Standard browser `WebSocket` JS API, ws+wss, event-loop-pumped. 7 commits (merged
from `feat/phase4-websocket`).

- `crates/oxibrowser-core/src/network/ws.rs` (new) — `run_ws_connection` background
  tokio task per socket; `tokio-tungstenite` 0.26 (already in workspace via CDP);
  `connect_async` (MaybeTlsStream), cmd/stream `select!`, id-keyed `WsEvent`
  (Open/Message/Close/Error), 10s connect timeout.
- `runtime.rs` — thread-locals `PENDING_WS` (id→`WsState::Live{obj}`|`Closed`),
  `WS_EVENT_RX`, `WS_REQ_TX`, `NEXT_WS_ID`; `SetWsChannel` command + `set_ws_channel`;
  WebSocket constructor (url/readyState/protocol/extensions/binaryType/bufferedAmount
  + on* properties + send/close/addEventListener); `drain_ws_events` + settle
  helpers (`settle_ws_open/message/close/error`, `ws_fire`); `drain_timers` calls
  `drain_ws_events` after the fetch drain.
- `session.rs` — `handle_ws_requests` bridge (mirrors `handle_fetch_requests`:
  try_recv + sleep().await, never blocking recv on the current-thread runtime);
  per-socket cmd channel registry; spawned on a dedicated OS thread.
- **`settle_to_idle` idle condition gained `pending_ws`** (any non-Closed socket) —
  a top-level `onopen` assigned *after* `new WebSocket()` still fires. This is the
  Phase 3 idle lesson applied; a design-review advisory caught the gap pre-spec.

### Phase 4 remaining (quick wins)
- **`Element.matches` / `Element.closest`** — `DomSnapshot::element_matches` /
  `element_closest` (reuse `node_matches_selector`); bound on element objects.
- **`URL.createObjectURL` / `revokeObjectURL`** — static methods on the URL
  constructor. Minimal: mints a `blob:https://oxibrowser.local/<id>` URL; revoke
  is a no-op (no payload registry). Unblocks SPAs that call it at init.
- **`AbortController` / `AbortSignal`** — JS-bootstrap classes
  (`WEB_COMPONENTS_BOOTSTRAP`): `aborted`/`reason`/`onabort`,
  `addEventListener('abort')`, `abort(reason)` propagation. Feature-detect +
  working abort events. **fetch abort-wiring NOT done** (objects exist; fetch
  does not yet read `signal.aborted`) — see Follow-ups.

### Verification
- core lib: **429 tests pass** (incl. 4 WebSocket echo-server acceptance tests +
  matches/closest + createObjectURL + AbortController), 2 ignored.
- `cargo clippy -p oxibrowser-core --all-targets -- -D warnings` clean.
- `setInterval(0)` gate intact (Phase 3 regression stays green).
- ⚠️ **Full `cargo test --workspace` NOT run** — `/Volumes/MERCURY` repeatedly hit
  100% disk (`target/debug/deps` is 16G; incremental/examples/build had to be
  deleted twice mid-session). core lib used as the green gate. cdp/webapi are
  unaffected by the WS/core changes but should be run once disk is freed.

---

## Follow-ups (Phase 4 incomplete)

1. **fetch abort-wiring** — `fetch(url, { signal })` should reject with an
   `AbortError` when `signal.aborted` is true at call time, and ideally cancel
   the in-flight background task on `abort()`. Currently the objects exist but
   fetch ignores `signal`. Touch `fetch_fn` (~`runtime.rs:1900`).
2. **`FormData` + file upload** — `new FormData()`, `.append(name, value[, file])`,
   multipart body in `fetch`/XHR. Needs `Blob`/`File` first.
3. **`Blob` constructor + payload registry** — `new Blob([parts], {type})`,
   `.size`/`.type`/`.text()`/`.arrayBuffer()`. Backs `URL.createObjectURL`
   (currently URL-only) and FormData.
4. **canvas 2D** — `getContext('2d')`, drawing commands, `toDataURL`/`toBlob`.
   Render integration; heavy.
5. **real Shadow DOM + customElement lifecycle** — current `attachShadow` is a
   JS-shim (feature-detect only); needs RenderDocument integration +
   `connectedCallback`/`disconnectedCallback`/`attributeChangedCallback`.
6. **WebSocket binary send** — `send(Uint8Array)` currently falls back to
   `toString` (text). Binary **receive** works (byte array; standard is
   ArrayBuffer). `binaryType:'blob'` unsupported (arraybuffer only).
7. **WebSocket wss in tests** — implemented (MaybeTlsStream), but only ws://echo
   is tested (self-signed cert overhead skipped).

---

## Roadmap (Phase 5–9, from `docs/superpowers/specs/2026-08-07-chrome-parity-roadmap.md`)

- **Phase 5 — CDP completeness**: `Emulation` (viewport/UA/timeout), sessionId
  multiplex, `exceptionThrown`, `console.entryAdded`, `Network.*` events,
  `Network.webSocketFrameCreated/Closed/Error` (emit from the WS background task).
  CDP is what lets Playwright *drive* the engine; grows per capability phase.
- **Phase 6 — network correctness**: CORS preflight, cookie expiry/domain,
  proxy, auth, redirects semantics.
- **Phase 7 — render/hit-test fidelity**: integrate the live DOM into Blitz
  render, real fonts, layout-based hit-testing (currently box-model estimate).
- **Phase 8 — iframe / multi-frame**: nested browsing contexts.
- **Phase 9 — long-tail Playwright surface**: dialog, download, multi-tab,
  file-upload input.

---

## Environment constraints (important)

- **Disk exhaustion is the active blocker.** `/Volumes/MERCURY` (931G) hit 100%
  repeatedly. `target/debug/deps` alone is **16G**. Each full workspace build
  needs ~4G free beyond deps. Mitigations used this session: `rm -rf
  target/debug/incremental`, `target/debug/examples`, `target/debug/build`.
  Before any workspace run, free ≥5G. Consider `cargo clean` + accepting a long
  rebuild, or moving target off `/Volumes/MERCURY`.
- Build times: clean ~7min, incremental ~5–15s. clippy --all-targets ~1min.
- `tokio-tungstenite`, `futures` now core deps (`Cargo.toml`).

## Repo state

- `main`, 20 commits ahead of `origin/main`, **not pushed** (user decision).
- No feature branch active (`feat/phase4-websocket` merged + deleted).
- CHANGELOG `[Unreleased]` has Phase 1/2/3/4-WebSocket entries. Phase 4 quick-wins
  (matches/closest, createObjectURL, AbortController) are committed but **not yet
  in CHANGELOG** — add them.

## Technical notes (carry forward)

- **boa 0.20 API**: no `From<String> for JsValue` → `JsValue::from(JsString::from(s.as_str()))`.
  `ObjectInitializer::function` takes a **NativeFunction** directly, not a built
  `JsFunction` (skip `FunctionObjectBuilder::build()`). `obj.get/set` want a
  `PropertyKey` (use `JsString::from(key)`, not `&str`). `arr.into()` to JsValue
  is ambiguous → `JsValue::from(arr)`. `JsArray` is at `boa_engine::object::builtins::JsArray`.
- **thread_local! with boa GC roots** (`JsValue`/`JsObject`): `HashMap::new()`
  initializer is **not** const-evaluable when the value holds a GC root — drop
  the `const { }` wrapper (see `PENDING_FETCH` / `PENDING_WS`). `Option<...>` /
  `Cell::new(1)` ARE const.
- **Async plumbing pattern** (Phase 3 fetch + Phase 4 WS): JS thread is `!Send`
  (`boa Context`); all async work on background tokio tasks; bridge via std mpsc
  + thread-local registries; **never block on `recv()` inside the current-thread
  runtime** (starves spawned tasks → deadlock) — use `try_recv()` +
  `sleep().await`. `settle_to_idle` is the single idle condition; every new
  long-lived async resource must add its own `pending_*` flag or onopen-style
  handlers silently drop.
- **The settle/idle lesson is now load-bearing**: a top-level
  `const x = new WebSocket(url); x.onopen = cb;` only fires `cb` because
  `settle_to_idle` keeps spinning while any socket is CONNECTING. Any future
  async resource (SSE, WebRTC, IndexedDB) needs the same treatment.
- **ws_fire callback dispatch**: on*-style callbacks live at `"on"+eventName`
  (`onclose`, not `close`). A real bug the WS tests caught — on* silently never
  fired until fixed.
- **dbg eprintln discipline**: when debugging runtime.rs closures, the edit tool
  mis-targets `PUT N.=N` inside large functions on stale tags; prefer `PUT >N`
  (insert) over `PUT N.=N` (replace) for one-liners, and always re-read after an
  edit that touches a region you edited earlier in the session.

## Specs / plans written this session

- `docs/superpowers/specs/2026-08-08-phase3-async-fetch.md` (Phase 3, prior)
- `docs/superpowers/specs/2026-08-08-phase4-websocket.md` (Phase 4 WS design)
- `docs/superpowers/plans/2026-08-08-phase4-websocket.md` (7-task TDD plan)
- `docs/superpowers/specs/2026-08-08-phase4-progress-and-handoff.md` (this doc)

## Suggested next session entry

1. Free disk (`cargo clean` or move target).
2. Add CHANGELOG entries for matches/closest + createObjectURL + AbortController.
3. `cargo test --workspace` once to confirm cdp/webapi green.
4. Push the 20 commits (or squash-group by phase).
5. Pick the next Phase 4 follow-up (fetch abort-wiring is the highest-value,
   lowest-scope) or start Phase 5 CDP completeness.
