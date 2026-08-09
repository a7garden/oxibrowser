# Phase 4 + Phase 5 Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [`) syntax for tracking.

**Goal:** Finish every remaining Phase 4 Web API (WebSocket binary send, FormData+Blob, canvas 2D, Shadow DOM/customElements) and every Phase 5 CDP-completeness item (sessionId multiplex, Emulation, Log+console+exception events, Network events for JS fetch/XHR + webSocketFrame*, Page dialogs, DOM.* methods) so that a Playwright script can drive `oxibrowser serve` end to end.

**Architecture:** All Web APIs live in `crates/oxibrowser-core/src/js/runtime.rs` (single 11k-line file) and are registered via two patterns — (a) JS bootstrap `const X_BOOTSTRAP: &str = r#"..."#` + `ctx.eval(Source::from_bytes(X_BOOTSTRAP))` at ~:8480-8518, mirrored onto `globalThis` AND `globalThis.window` (distinct objects); (b) native `NativeFunction::from_closure(...)` + `context.register_global_callable(js_string!(NAME), arity, fn)`, constructors via `ObjectInitializer` chaining `.property().accessor().function()`. CDP domains live in `crates/oxibrowser-cdp/src/domains/*.rs`, dispatched from `mod.rs::dispatch` (`match domain { ... }`); events flow through `EventSender` (per-domain enabled flags + `send_*_event`). One `CdpSession` per WebSocket; `CdpEvent.session_id` exists but is never stamped today.

**Tech Stack:** Rust, `boa_engine` 0.20 (!Send Context on a dedicated thread), `wreq` HTTP, `tokio-tungstenite` WebSocket, hyper for the CDP server. No new JS engine, no Chromium.

## Global Constraints

- Pure Rust only — no V8, no Chromium, no C/C++ JS engine. New browser APIs are either JS-bootstrap strings or native boa closures.
- `window` ≠ `globalThis` here — every new global MUST be installed on BOTH `globalThis` and `globalThis.window` (rebuilt each navigation).
- TDD: every task writes a failing test first, watches it fail, implements minimal code, watches it pass. One conventional commit per task.
- `cargo test --workspace` green + `cargo clippy --workspace -- -D warnings` clean + `cargo fmt --all --check` clean after every task.
- The event-loop pump order is fixed: `eval → run_jobs → drain_timers(→drain_pending_fetch_responses→drain_ws_events) → settle_to_idle`. New async APIs must settle through this pump, not invent a new path.
- `FetchRequestMsg`/`FetchResponseMsg` cross the JS↔async boundary via mpsc; new body types must stay `Send`.

---

## Task 0 (FOUNDATION): fetch method/headers/body actually reach the wire

**Problem:** JS `fetch(url, {method, headers, body})` extracts method/headers/body into `FetchRequestMsg` (runtime.rs:2280-2331) but `session.rs::handle_fetch_requests` (:115-249) calls the **GET-only** `http_client.fetch(&url)` (:170) and ignores `request.method/headers/body`. So POST/PUT/DELETE + any body (including FormData) never leaves the machine. This blocks the login-form acceptance test and FormData.

**Files:**
- Modify: `crates/oxibrowser-core/src/network/client.rs` — add `HttpClient::request(url, method, headers, body) -> Result<Response>`.
- Modify: `crates/oxibrowser-core/src/session.rs:170` — dispatch via the new method instead of GET-only `fetch`.
- Test: `crates/oxibrowser-core/src/network/client.rs` (unit: method routing) + `crates/oxibrowser-core/src/session.rs` (integration: capture server asserts method+body).

**Interfaces:**
- Produces: `pub async fn HttpClient::request(&self, url: &Url, method: &str, headers: &[(String,String)], body: Option<Vec<u8>>) -> Result<Response>` — SSRF-checked, cookie-attached, response-cookie-stored, handles GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS.
- `FetchRequestMsg.body` changes `Option<String>` → `Option<Vec<u8>>` (UTF-8 for string bodies). Update the single producer (runtime.rs:2325-2331) and single consumer (session.rs).

- [ ] **Step 1: RED** — add `network/client.rs` test `request_uses_method_and_body`: spin a TcpListener capture server (bind 127.0.0.1:0) that reads the raw request line + body into `Arc<Mutex<Option<(String,String)>>`, responds `HTTP/1.1 200 OK`. Call `HttpClient::request` with POST + a body, assert captured method == "POST" and body echoes. Watch fail (method absent).
- [ ] **Step 2: GREEN** — implement `HttpClient::request` (reusing the `intercept` body via `RequestBuilder` per-method, apply headers + cookie + SSRF + store cookies).
- [ ] **Step 3** — change `FetchRequestMsg.body` to `Option<Vec<u8>>`, update producer/consumer; rewrite session.rs:170 to call `http_client.request(&url, &request.method, &request.headers, request.body)`.
- [ ] **Step 4: RED→GREEN** — add session.rs integration test with the same capture server wired through a real `JsRuntime`+`set_fetch_channel`→`handle_fetch_requests`, assert a JS `fetch(url,{method:'POST',body:'hi'})` delivers POST+"hi" on the wire.
- [ ] **Step 5: commit** `feat(core): route fetch method/headers/body to the wire (was GET-only)`.

---

## Task 1: WebSocket binary send

**Problem:** `WebSocket.prototype.send` (runtime.rs:2694-2713) coerces every arg to `WsData::Text`; ArrayBuffer/TypedArray/Blob never produce a binary frame. The wire layer (`network/ws.rs` `WsData::Binary`) already handles binary both ways.

**Files:** Modify `crates/oxibrowser-core/src/js/runtime.rs` send closure (~:2694-2713). Test: runtime.rs `tests`.

- [ ] **RED** — test: build a JsRuntime, set a ws req channel, `new WebSocket('ws://127.0.0.1:1/x')`, then `ws.send(new Uint8Array([1,2,3]))`; recv the `WsReqMsg::Send{data}`; assert `data == WsData::Binary(vec![1,2,3])`. Watch fail (gets Text).
- [ ] **GREEN** — in send closure: if arg is a JsValue::Object, probe for typed-array byte data (`.buffer`/`byteLength`/`byteOffset`) or a Blob `[Symbol.iterator]`/`size`+`arrayBuffer()`; extract bytes → `WsData::Binary`; else fall back to Text. (Match the receive side's existing byte-extraction helper at runtime.rs:1916-1921.)
- [ ] **commit** `feat(core): WebSocket binary send (ArrayBuffer/TypedArray)`.

---

## Task 2: sessionId multiplex (flat-protocol)

**Problem:** `CdpEvent::new` always sets `session_id: None` (protocol.rs:70). Playwright connects to the browser WS, calls `Target.setAutoAttach({flatten:true})`, then routes all target commands/events by `sessionId` — but our events carry none. The response echo (session.rs:230/236) already works; only events are broken.

**Files:** Modify `crates/oxibrowser-cdp/src/event.rs` (`EventSender` gains `attached_session_id: Arc<RwLock<Option<String>>>`; `send_event` stamps it); `domains/target.rs` (`attachToTarget`/`setAutoAttach` call `ctx.events.set_session_id(id)` and emit `attachedToTarget` carrying that id). Test: event.rs unit + target.rs.

- [ ] **RED** — event.rs test: `EventSender`, `set_session_id("S1")`, `send_event("X", json!({}))`, drain receiver, assert `event.session_id == Some("S1")`; and before set, assert None.
- [ ] **GREEN** — add field + setter + stamp in `send_event`; wire target handlers to set it on attach.
- [ ] **commit** `feat(cdp): stamp sessionId on CDP events (flat-protocol multiplex)`.

---

## Task 3: Emulation domain (setDeviceMetricsOverride)

**Problem:** No `Emulation` domain (mod.rs:58-72 has no arm). Playwright/Puppeteer set viewport via `Emulation.setDeviceMetricsOverride`.

**Files:** Create `crates/oxibrowser-cdp/src/domains/emulation.rs`; register `pub mod emulation;` + `"Emulation" => emulation::handle(...)` in mod.rs. Test: emulation.rs.

- [ ] **RED** — test: `handle("setDeviceMetricsOverride", {width:375,height:812,deviceScaleFactor:2,mobile:true})` returns `Ok(Some({}))`; `clearDeviceMetricsOverride` likewise; unknown method → error.
- [ ] **GREEN** — implement `handle` dispatching the two methods (+ `setVisibleSize`), storing the override on a shared `Arc<RwLock<Option<DeviceMetrics>>>` attached to `EventSender` or `DispatchContext` so later viewport reads can use it. (Minimal: store + acknowledge; wiring into the render viewport is a follow-up but the ack is what Playwright blocks on.)
- [ ] **commit** `feat(cdp): Emulation.setDeviceMetricsOverride/clear`.

---

## Task 4: Log domain + consoleAPICalled + exceptionThrown live events

**Problem:** No `Log` domain; `console.*` does not emit `Runtime.consoleAPICalled`; thrown exceptions do not emit `Runtime.exceptionThrown`. Playwright surfaces these for debugging.

**Files:** Create `domains/log.rs`; modify `event.rs` (`log_enabled` flag + `send_log_event`); modify `domains/runtime.rs` (`Log.enable`→flag, `Runtime.enable` already exists); thread console/exception hooks from the JS runtime into the event sender. JS console already exists (runtime.rs:2125-2159) — add a callback channel `JsRuntime::set_console_sink` mirroring `set_fetch_channel`, drained to emit `Runtime.consoleAPICalled`. Test: log.rs + a runtime test that `console.log('x')` (with Runtime+Log enabled) emits the event.

- [ ] **RED** — runtime test: console.log('hi', 42) routes to a sink; assert the captured args. log.rs test: `Log.enable` flips the flag; `Log.entryAdded`-shaped event round-trips.
- [ ] **GREEN** — Log domain (enable/disable/entryAdded clear), `send_log_event`; console sink wiring; exception sink wiring (`Runtime.exceptionThrown` on eval error).
- [ ] **commit** `feat(cdp): Log domain + consoleAPICalled + exceptionThrown events`.

---

## Task 5: FormData + Blob + fetch multipart serialization

**Problem:** No `FormData`, no `Blob`. Form uploads are core to automation. Builds on Task 0's byte-body plumbing.

**Files:** New `const FORMDATA_BLOB_BOOTSTRAP` + eval in runtime.rs (~:8518). fetch_fn body extraction (~:2297-2305) extended to detect FormData/Blob. Test: runtime.rs.

**Interfaces (JS):**
- `new Blob(parts, opts)` → object with `size`, `type`, `arrayBuffer()` (resolves Vec<u8>), `text()`.
- `new FormData()` → `append(name, value, filename?)`, `get/set/has/delete/entries`, iterable.
- fetch: if body is FormData → serialize `multipart/form-data; boundary=...`; if Blob → raw bytes with its `type` as Content-Type default.

- [ ] **RED** — test: `new FormData(); fd.append('a','1'); fd.append('file', new Blob([0,1,2]), 'x.bin')`; fetch it via capture server; assert request Content-Type starts `multipart/form-data; boundary=` and body contains `name="a"` and `filename="x.bin"` and the boundary delimiters. Blob unit: `new Blob([1,2,3]).size === 3` and `.arrayBuffer()` length 3.
- [ ] **GREEN** — JS-bootstrap Blob + FormData (store entries as `{name, type:'string'|'blob', value, filename}`); a native helper to serialize FormData→bytes+boundary (or pure-JS with a known boundary); fetch_fn: when body is FormData, build multipart bytes; when Blob, use its bytes.
- [ ] **commit** `feat(core): FormData + Blob + multipart fetch body`.

---

## Task 6: canvas 2D context shim

**Problem:** No HTMLCanvasElement.getContext, no CanvasRenderingContext2D. SPAs/captcha/analytics call these at load. Goal: existence + no-throw + best-effort `toDataURL`/`getImageData`, not real rasterization.

**Files:** New `const CANVAS_BOOTSTRAP` + eval. JS-only: `HTMLCanvasElement.prototype.getContext('2d'|'webgl')` returns a context object with the 2D API surface (fillRect, fillText, arc, beginPath, fill, stroke, drawImage, measureText→TextMetrics, save/restore, translate/scale/rotate, fillStyle/strokeStyle/font setters) as recording no-ops; `toDataURL()` returns `data:,`; `toBlob()` resolves a Blob. Test: runtime.rs.

- [ ] **RED** — test: `var c = document.createElement('canvas'); var ctx = c.getContext('2d'); ctx.fillRect(0,0,10,10); ctx.fillStyle='#fff'; c.toDataURL()` returns a `data:` string; `typeof ctx.measureText === 'function'`; `c.getContext('webgl')` truthy.
- [ ] **GREEN** — bootstrap defines the prototype methods (no-ops returning expected types: measureText→{width:0}, getImageData→{data: Uint8ClampedArray, width,height}, etc.), installs `HTMLCanvasElement` if absent.
- [ ] **commit** `feat(core): canvas 2D context shim (no-throw + toDataURL)`.

---

## Task 7: Network events for JS fetch/XHR + Network.webSocketFrame*

**Problem:** `Network.*` lifecycle events fire only for top-level navigation (`emit_navigation_events`). JS-initiated fetch/XHR/WebSocket frames emit nothing — Playwright's request inspection/mock and `webSocketFrameSent/Received` are blind. Builds on Task 0 (fetch path) + Task 2 (sessionId).

**Files:** Plumb a network-event sink from the JS runtime (fetch_fn / xhr send / ws send+recv) into `EventSender`: `Runtime`→`set_network_sink`; in fetch_fn dispatch emit `Network.requestWillBeSent` (pre-send, id=url hash) and on response `Network.responseReceived`+`loadingFinished`; WS send/recv emit `Network.webSocketFrameSent`/`Received`. Test: a runtime test that fetch (against capture server) with Network enabled produces the three events; ws frame test produces webSocketFrameSent/Received.

- [ ] **RED** — test: enable Network, fetch capture-server, assert three Network events captured on a sink; ws echo-server, assert webSocketFrameSent + Received.
- [ ] **GREEN** — sink channel + event emission at the JS-side send/receive points; map to CDP `Network.requestWillBeSent/responseReceived/loadingFinished` + `webSocketFrameSent/Received` shapes.
- [ ] **commit** `feat(cdp): Network lifecycle events for JS fetch/XHR + webSocketFrame*`.

---

## Task 8: Page.handleJavaScriptDialog + alert/confirm/prompt

**Problem:** No dialog handling. `alert/confirm/prompt` either throw or block; Playwright uses `Page.handleJavaScriptDialog`.

**Files:** runtime.rs: bootstrap `window.alert/confirm/prompt` that queue a pending dialog (id) and resolve when a CDP `handleJavaScriptDialog` arrives (or auto-default). `domains/page.rs`: `handleJavaScriptDialog` (accept/dismiss + promptText). Plumb a dialog channel `JsRuntime::set_dialog_sink`. Test: runtime test that alert() returns and a queued dialog is observable; page.rs test for the handler.

- [ ] **RED** — test: `alert('x')` no-throw and emits a dialog event on sink; `prompt('q')` resolves the CDP-supplied text; `confirm('q')` resolves accept=true.
- [ ] **GREEN** — dialog queue + resolve on `Page.handleJavaScriptDialog`; JS bootstrap defaults (alert→noop, confirm→false, prompt→null) when no handler.
- [ ] **commit** `feat(core): alert/confirm/prompt + Page.handleJavaScriptDialog`.

---

## Task 9: Shadow DOM + customElements lifecycle

**Problem:** `WEB_COMPONENTS_BOOTSTRAP` has stubs only: `attachShadow` returns a `createDocumentFragment()` throwaway; `customElements.define` registers a name but never upgrades elements or fires `connectedCallback`/`disconnectedCallback`/`attributeChangedCallback`; no `<slot>`. Goal: real-enough lifecycle — defining registers, parsed/created elements get upgraded and fire connected/disconnected when appended/removed (via the existing DOM mutation path), attributeChanged for `observedAttributes`.

**Files:** runtime.rs (bootstrap upgrade of the existing stubs) + `dom_snapshot.rs` (expose a hook so element insertion/removal can trigger lifecycle callbacks — the JS side observes via MutationObserver or a native mutation callback). Test: runtime.rs.

- [ ] **RED** — test: `customElements.define('x-foo', class extends HTMLElement { connectedCallback(){ window.__connected=(window.__connected||0)+1 } })`; `document.body.appendChild(document.createElement('x-foo'))` then pump → `window.__connected===1`; remove → `disconnectedCallback` fires; `observedAttributes=['a']` + `setAttribute('a','1')` → attributeChangedCallback.
- [ ] **GREEN** — registry upgrade: on define, scan existing nodes and upgrade; on append/remove (hook the DOM mutation bridge), fire callbacks; attachShadow stores a real ShadowRoot object the host renders into (render fidelity is a separate phase — here we make the JS contract + lifecycle correct).
- [ ] **commit** `feat(core): customElements lifecycle + attachShadow/ShadowRoot`.

---

## Task 10: DOM.* missing methods

**Problem:** `DOM.*` surface is partial. Survey Playwright/Puppeteer usage, add the high-value missing methods.

**Files:** `domains/dom.rs` + runtime.rs/dom_snapshot.rs as needed. Survey first (read-only), then implement by impact. Likely: `DOM.querySelector`, `querySelectorAll`, `getOuterHTML`, `removeNode`, `setNodeValue`, `requestNode`, `resolveNode`, `describeNode` completeness, `getBoxModel`/`getContentQuads` (layout-based — may stub).

- [ ] **Survey** — list Playwright-used DOM.* methods vs current `dom::handle` arms; pick the gap set.
- [ ] **RED→GREEN per method** — one test each, conventional commit grouping.
- [ ] **commit** `feat(cdp): expand DOM.* method coverage`.

---

## Task 11 (VERIFY): Playwright acceptance probe + full gate

- [ ] **Probe** — minimal puppeteer-core script against `cargo run -- serve`: connect, Target.setAutoAttach, setDeviceMetricsOverride, navigate to a local mock React-ish page, evaluate `document.title`, click, screenshot. Capture the first real blocker, fix or file.
- [ ] **Gate** — `cargo test --workspace` green, `cargo clippy --workspace -- -D warnings` clean, `cargo fmt --all --check` clean.
- [ ] **Docs** — update CHANGELOG `[Unreleased]` with all Phase 4+5 items; update roadmap status.
- [ ] **Final commit/tag decision** — if the Playwright probe passes end to end, tag per roadmap §6.

---

## Self-review notes

- Spec coverage: Phase 4 remaining (WS binary, FormData/Blob, canvas, Shadow DOM) = Tasks 1,5,6,9. Phase 5 (Emulation, sessionId multiplex, Network.* events, webSocketFrame*) = Tasks 2,3,4,7 + Log/dialog/DOM = 4,8,10. Roadmap §3 Phase 5 list fully covered.
- Foundational discovery: Task 0 (fetch-body) is NOT in the roadmap's Phase 4/5 list but is a blocker discovered during planning — it gates FormData (Task 5), the acceptance login POST (Task 11), and Network fetch events (Task 7). Sequenced first.
- Dependency chain: Task 0 → 5,7. Task 2 → 7 (sessionId stamping). All others independent. Execute 0 first, then fan out.
- `FetchRequestMsg.body: Option<Vec<u8>>` is the only cross-cutting type change; producer/consumer both in known single sites.
