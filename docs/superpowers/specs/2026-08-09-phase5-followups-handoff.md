# Phase 4+5 Follow-ups Handoff — OxiBrowser → Headless-Chrome Parity

> **Status:** ✅ Sub-session A (CoreEvent sink + emitters), Sub-session B
> (lifecycle callbacks + geometry methods + ShadowRoot slot composition) all
> complete. The only remainder is screenshot rasterization of shadow content
> (`capture_png`), which is gated on the external Blitz render engine (§4).
>
> **Phase 4+5 core:** ✅ complete — see `docs/superpowers/plans/2026-08-09-phase4-5-completion.md`.
> **Branch:** `main`
> **Roadmap:** `docs/superpowers/specs/2026-08-07-chrome-parity-roadmap.md`

---

## 1. Where Phase 4+5 Left Off

### Shipped (this session, `8269eb3..a562937`)

| Commit | What |
|--------|------|
| `4337389` | docs: Phase 4+5 completion implementation plan |
| `89ec2b8` | feat(core): route fetch method/headers/body to the wire (was GET-only) |
| `92d64b8` | feat(core): WebSocket binary send (ArrayBuffer/TypedArray/Array) |
| `2dc664c` | feat(cdp): stamp sessionId on CDP events (flat-protocol multiplex) |
| `15c4a8f` | feat(cdp): Emulation domain (setDeviceMetricsOverride/clear) |
| `632376d` | feat(core): FormData + Blob + multipart fetch body |
| `2461f12` | feat(core): canvas 2D context shim (getContext + toDataURL) |
| `9ce005f` | feat(core): upgrade createElement'd custom elements (prototype + ctor) |
| `b39d989` | feat(cdp): expand DOM.* method coverage |
| `a562937` | feat: Phase 4+5 completion (Dialog MVP + Log stub + probe + CHANGELOG) |

### Acceptance probe — PASSES

A raw-CDP probe (`/tmp/oxi-probe/probe.ts`, bun + WebSocket) against
`oxibrowser serve --allow-private-ips` confirms end-to-end:

- `Target.setAutoAttach` → `attachedToTarget` with `sessionId` ✅
- `Runtime.enable` / `Page.enable` ✅
- `Emulation.setDeviceMetricsOverride` ✅
- `Page.navigate` runs the page's inline `<script>` ✅ (window.__probe === "ran")
- `getElementById` works during nav scripts ✅ (window.__h1 === "hello")
- `document.title` === "OxiProbe" ✅
- 4/6 events carry `sessionId`; the 2 unstamped are root-level `Target.targetCreated`
  and `Target.attachedToTarget` (correct — they announce the session).

### ⚠ Build gotcha (cost ~1h of misdiagnosis)

The `oxibrowser` **binary** requires the `browser` feature:

```bash
cargo build --features browser --bin oxibrowser   # CORRECT — relinks the bin
cargo build -p oxibrowser                          # WRONG — does NOT relink the bin
```

`cargo build -p oxibrowser` reports "Finished" with no recompile of the binary
because the `[[bin]]` has `required-features = ["browser"]`. The stale binary
mtime persists and the acceptance probe sees days-old behaviour. Verify after
every build: `stat -f "%Sm" target/debug/oxibrowser`.

### CI gates (must pass before each commit)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# probe (after a --features browser build):
bash /tmp/oxi-probe/run2.sh   # mock + serve + probe orchestration
```

### Patterns to reuse

| Pattern | Established at | Reuse for |
|---|---|---|
| JS bootstrap (`const X_BOOTSTRAP: &str = r#"..."#` + `ctx.eval`) installed on globalThis AND window | `runtime.rs` FORMDATA_BLOB / CANVAS / DIALOG bootstraps | any new pure-JS Web API |
| Native `NativeFunction::from_closure` + `register_global_callable` / `ObjectInitializer` | `runtime.rs` fetch/WebSocket/XHR | new native Web APIs needing ctx |
| mpsc channel JS thread ↔ async main thread (`set_fetch_channel`, `set_ws_channel`) | `runtime.rs` / `session.rs` | the **CoreEvent sink** below |
| CDP domain: new file in `domains/`, `pub mod` in `mod.rs`, match arm in `dispatch` | emulation.rs / log.rs | new CDP domains |
| `EventSender` per-domain enabled flags + `send_*_event` gating | `event.rs` | new event-gated domains |
| Multi-thread `#[tokio::test(flavor="multi_thread")]` for wreq + capture-server tests | `client.rs::request_sends_method_and_body_on_the_wire` | any wreq round-trip test |

---

## 2. Remaining Scope

One paragraph: the 4 stubbed Phase 5 items (`Runtime.consoleAPICalled`,
`Runtime.exceptionThrown`, `Network.*` for JS fetch/XHR, `Page.javascriptDialogOpening`
+ event-driven dialog, `Log.entryAdded`) are all blocked on a single missing
plumbing — a **core→CDP CoreEvent sink**. Build that sink (sub-session A), then
each follow-up is a small, independent emitter. Separately, Phase 7 (unified
live DOM) gates `connectedCallback` on insertion, slot rendering, and the
layout-geometry DOM methods (sub-session B).

### ⚠ Critical reconciliation

- The Phase 4+5 plan doc listed "Log domain + consoleAPICalled + exceptionThrown"
  as one task. In practice only the **Log domain handler** shipped (enable/disable
  ack); the **event emission** is the unsunk follow-up.
- "Network events for JS fetch/XHR" shipped only for **navigation**
  (`emit_navigation_events` / `emit_response_events` in `domains/network.rs`).
  JS-initiated fetch/XHR/WebSocket frames emit nothing.
- Shadow DOM shipped only **createElement-upgrade**; lifecycle callbacks on
  insertion need Phase 7.

### Sub-sessions

| Sub-session | Scope | Est. commits | Difficulty |
|---|---|---|---|
| **A. CoreEvent sink + 4 emitters** | sink plumbing + consoleAPICalled + exceptionThrown + Network fetch/XHR + webSocketFrame* + Page.javascriptDialogOpening + Log.entryAdded | 4–6 | 🟡 |
| **B. Phase 7 geometry + lifecycle** | unified live DOM (or a targeted hook); connectedCallback on appendChild; getBoxModel/getContentQuads/getNodeForLocation via LayoutEngine; slot rendering | 4–8 | 🔴 |

---

## 3. Sub-session A — CoreEvent sink + event emitters

### 3.1 The bottleneck: no core→CDP event channel

`oxibrowser-core` cannot depend on `oxibrowser-cdp`, so the JS thread (which
lives in core) has no handle to the CDP `EventSender`. Today the only JS→main
channels are per-feature (`set_fetch_channel`, `set_ws_channel`, localStorage).
Add **one** generic sink and all 4 follow-ups fall out.

### 3.2 Design

**Core side (`oxibrowser-core`):**

```rust
// crates/oxibrowser-core/src/js/runtime.rs (near FetchRequestMsg, ~line 400)
/// A core-originated event destined for the CDP layer (or any observer).
/// Core cannot name CDP types, so this is a neutral enum the CDP drainer
/// translates into CDP events.
pub enum CoreEvent {
    Console { level: ConsoleLevel, args: Vec<String> },        // from console_fn
    Exception { message: String, stack: Option<String> },      // from evaluate() error path
    FetchRequest { request_id: String, url: String, method: String,
                   headers: Vec<(String,String)>, post_data: Option<Vec<u8>>,
                   timestamp: f64 },
    FetchResponse { request_id: String, url: String, status: u16,
                    mime_type: String, timestamp: f64 },
    FetchLoadingFinished { request_id: String, timestamp: f64 },
    WsFrame { direction: WsDirection, request_id: String, opcode: u8,
              data: String, timestamp: f64 },
    Dialog { dialog_type: DialogType, message: String, default_value: Option<String> },
}
```

- `JsRuntime` gains `event_tx: Arc<RwLock<Option<mpsc::Sender<CoreEvent>>>>` +
  `pub fn set_event_sink(&mut self, tx)` (mirrors `set_fetch_channel`).
- The JS thread reads it via a thread-local set from the SetEventSink command
  (mirror `SetFetchChannel` at `runtime.rs:316`).
- `console_fn` (`runtime.rs:2177`) pushes `CoreEvent::Console` alongside the
  existing `output` buffer write. The args are already stringified there.
- `evaluate()`'s error path (`runtime.rs:~1019`, the `ctx.eval(source)` result)
  pushes `CoreEvent::Exception` when an exception is returned.
- `fetch_fn` (`runtime.rs:2233`): push `FetchRequest` before the channel send
  (~line 2353), and `drain_pending_fetch_responses` (`runtime.rs:1720`) pushes
  `FetchResponse` + `FetchLoadingFinished` when a response settles. **Reuse the
  existing `request.id`** as the CDP `requestId` (format it as `"oxi-{id}"`).
- `drain_ws_events` (`runtime.rs:~1940`): on `WsEvent::Message`, push
  `CoreEvent::WsFrame` (direction depends on whether it was a send echo vs
  receive — the send side is in the send closure at `runtime.rs:~2728`).
- `DIALOG_BOOTSTRAP`'s `alert/confirm/prompt` become native closures (move out
  of the JS bootstrap) that push `CoreEvent::Dialog` and resolve via a pending-
  dialog registry (mirrors `PENDING_FETCH`) that `Page.handleJavaScriptDialog`
  resolves.

**CDP side (`oxibrowser-cdp`):**

```rust
// crates/oxibrowser-cdp/src/session.rs — in CdpSession::new, after browser.new_session():
let (core_tx, core_rx) = std::sync::mpsc::channel::<CoreEvent>();
session.set_event_sink(core_tx);   // Session exposes this via js_runtime accessor
// spawn a drainer on the CdpSession's tokio runtime:
let events = self.event_sender.clone();
tokio::spawn(async move {
    loop {
        while let Ok(ev) = core_rx.try_recv() {
            emit_core_event(&events, ev);   // match -> send_runtime_event / send_network_event / send_page_event
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
});
```

`emit_core_event` maps:
- `Console` → `Runtime.consoleAPICalled` (`{type, args:[{type:"string",value}], executionContextId:1}`)
  (gated by `runtime_enabled`)
- `Exception` → `Runtime.exceptionThrown` (`{timestamp, exceptionDetails:{...}}`) (gated)
- `FetchRequest/Response/LoadingFinished` → the three `Network.*` events (gated
  by `network_enabled`) — **shape them exactly like `emit_navigation_events`
  (`domains/network.rs:250`)**.
- `WsFrame` → `Network.webSocketFrameSent`/`Received` (gated)
- `Dialog` → `Page.javascriptDialogOpening` (`{url, message, type, defaultPrompt}`)
  (gated by `page_enabled`).

### 3.3 File layout

```
crates/oxibrowser-core/src/js/runtime.rs   # CoreEvent enum + JsRuntime::set_event_sink + console/exception/fetch/ws/dialog pushes
crates/oxibrowser-core/src/session.rs      # Session exposes js_runtime.set_event_sink in new()
crates/oxibrowser-cdp/src/session.rs       # CdpSession::new spawns the CoreEvent drainer
crates/oxibrowser-cdp/src/event.rs         # (optional) send_log_event + log_enabled flag for Log.entryAdded
```

### 3.4 Key code anchors (read these first)

| Anchor | Location | Why |
|---|---|---|
| `console_fn!` macro | `runtime.rs:2177` | where to push `Console` |
| `evaluate()` eval+settle block | `runtime.rs:~1002–1019` | where to push `Exception` |
| `fetch_fn` dispatch | `runtime.rs:~2325` (FetchRequestMsg build) + `:2353` (send) | where to push `FetchRequest` |
| `drain_pending_fetch_responses` | `runtime.rs:1720` | where to push `FetchResponse`/`LoadingFinished` |
| `drain_ws_events` | `runtime.rs:~1940` | where to push `WsFrame` |
| `SetFetchChannel` cmd | `runtime.rs:316` | template for `SetEventSink` |
| `JsRuntime::set_fetch_channel` | `runtime.rs:583` | template for `set_event_sink` |
| `handle_fetch_requests` | `session.rs:115` | existing JS↔async channel pattern |
| `emit_navigation_events` | `domains/network.rs:250` | exact Network.* event shape to reuse |
| `CdpSession::new` / `run` | `session.rs:60` / `:95` | where the drainer task attaches |
| `EventSender` flags + `send_*_event` | `event.rs:48–146` | gating model |

### 3.5 Suggested commit order

1. `feat(core): CoreEvent sink + JsRuntime::set_event_sink` (enum + channel +
   `Session` wiring + a unit test that console.log pushes a Console event).
2. `feat(cdp): drain CoreEvent → Runtime.consoleAPICalled + exceptionThrown`
   (+ `Log.entryAdded`).
3. `feat(cdp): Network lifecycle events for JS fetch/XHR + webSocketFrame*`.
4. `feat(core): event-driven alert/confirm/prompt + Page.javascriptDialogOpening`
   (move DIALOG_BOOTSTRAP functions to native closures that push `Dialog` and
   block on a pending registry resolved by `Page.handleJavaScriptDialog`).

Each ships with a focused test and the raw-CDP probe re-run as regression.

---

## 4. Sub-session B — Phase 7 (geometry + lifecycle)
**Status (2026-08-09):** ✅ all four items shipped — lifecycle callbacks,
geometry methods, and ShadowRoot slot composition (the flattened-tree compose
pass makes shadow/slot content visible to every DomSnapshot-backed read). The
sole remainder is screenshot rasterization of shadow content via `capture_png`,


| Item | Status | How |
|---|---|---|
| `connectedCallback`/`disconnectedCallback` on DOM insertion | ✅ done | Native `appendChild`/`remove` hooks on the render-doc element path call `__oxi_fire_connected`/`__oxi_fire_disconnected` (JS helpers in `WEB_COMPONENTS_BOOTSTRAP`); fires on the appended/removed node (+ best-effort subtree walk). Verified by `test_custom_element_lifecycle_callbacks`. |
| `attributeChangedCallback` for `observedAttributes` | ✅ done | Render-doc `setAttribute` hook captures the old value and calls `__oxi_fire_attr_changed`, gated by `observedAttributes` inside the helper. |
| `getBoxModel` / `getContentQuads` / `getNodeForLocation` | ✅ done | `domains/dom.rs` methods backed by `LayoutEngine::compute_rect(snapshot, node_id)`. Verified end-to-end via raw-CDP probe (`getBoxModel` 8-pt quad + dims; `getContentQuads`; `getNodeForLocation` returns a nodeId). |
| slot rendering / real ShadowRoot composition | ✅ done (DomSnapshot-level) | Real Shadow DOM composition for every DomSnapshot-backed read (CDP `DOM.*`, `getBoxModel`/`getContentQuads`/`getNodeForLocation`, `OXI.*`, `extract`, accessibility, `LayoutEngine`). A `SHADOW_ROOTS` registry + native `__oxi_attach_shadow` (runtime.rs) records shadow children; `compose_shadow_trees` (dom_snapshot.rs) is a post-pass in `from_render_document` that merges each host's shadow subtree and distributes light-DOM children into `<slot>` positions by name (default + named; non-matching dropped; slot fallback) — the standard flattened tree, no Blitz change. Verified by `test_shadow_dom_slot_composition` + `test_shadow_dom_named_slot_composition`. **Only screenshot rasterization (`capture_png`) is still Blitz-gated** — `blitz_dom::BaseDocument` (external `blitz-dom 0.3.0-beta.1`) has no shadow model; that one path is a documented follow-up, not part of this deliverable's read/composition surface. |

The DOM-methods survey (`history://DomMethodSurvey`) enumerated the full gap;
the 9 JS-eval/Snapshot-feasible methods shipped, the 3 geometry ones are here.

---

## 5. Risks & Gotchas

| Risk | Status | Follow-up |
|------|--------|-----------|
| CoreEvent drainer busy-loops / leaks when session closes | open | drop the Sender on `CdpSession::run` exit; the drainer's `try_recv` returns Err and it exits. Add a shutdown oneshot mirroring `handle_fetch_requests`. |
| JS thread pushing CoreEvent while no sink set (no CDP attached) | open | `event_tx` is `Option`/`RwLock<Option<Sender>>`; push is a no-op when None (like the CLI `fetch` path with no channel). |
| consoleAPICalled `args` should preserve type info (objects) | accepted for v1 | stringify in core (already done in console_fn); richer RemoteObject serialization is a later refinement. |
| Dialog blocking the JS thread could deadlock the pump | open | resolve the pending dialog from the CDP drainer (async side) via a shared cell; the JS thread's `drain_*` polls it. Never block on a blocking `recv()` inside the JS thread. |
| Stale binary (browser feature) | **documented** | always build with `--features browser --bin oxibrowser`; the probe's first "failure" is this trap. |
| `request.id` (u64) vs CDP `requestId` (string) | open | format as `"oxi-{id}"`; ensure fetch-FetchRequest and fetch-FetchResponse use the same id so clients can correlate. |

---

## 6. Out of Scope (explicit)

- Real rasterization for canvas (recording no-ops are sufficient for automation).
- Real WebGL rendering (no-op context surface is sufficient).
- Full cookie expiry / PSL / CHIPS (Phase 6, network correctness).
- CORS + preflight (Phase 6).
- Anti-bot challenge solving (roadmap non-goal).
- Pixel-perfect Chrome rendering parity (roadmap non-goal).

---

## 7. Quick Reference: probe

The acceptance probe lives in `/tmp` (ephemeral). To make it permanent, move
`mock.ts` + `probe.ts` + `run2.sh` into `crates/oxibrowser/tests/` or a
`probe/` dir and wire as an ignored integration test (needs
`--features browser` + bun). Re-run anytime:

```bash
cargo build --features browser --bin oxibrowser
MOCK_PORT=18080 bun mock.ts &
./target/debug/oxibrowser serve --port 19321 --allow-private-ips &
bun probe.ts 19321 18080   # expect 10/10 PASS, 4/6 events sessionId-stamped
```

---

End of handoff. Read this + `docs/superpowers/plans/2026-08-09-phase4-5-completion.md`
+ the anchors in §3.4 and you're ready to start Sub-session A.
