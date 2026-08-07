# Phase 4 — WebSocket (full standard)

**Date:** 2026-08-08
**Status:** Approved (design); implementation pending
**Roadmap:** Phase 4 — Missing Web APIs (SPA impact), second item after `matchMedia`
**Scope:** Native `WebSocket` JS Web API, full standard surface, ws + wss.

## Goal

Implement the standard browser `WebSocket` API so SPAs that open realtime
connections (chat, streaming, live dashboards, collaborative tools) run without
feature-detection throws and without deadlocking the JS thread.

`fetch()` and `xhr.send()` became non-blocking in Phase 3. `WebSocket` adds a
**second, long-lived** async resource: a connection that stays open and pushes
messages back to JS at arbitrary times. The engine must pump those server-pushed
events on the event loop.

## Background & constraints

- **Pure Rust.** No Chromium, no V8. JS via `boa_engine` 0.20; rendering via Blitz.
- **JS thread is `!Send`.** `boa` `Context` lives on one OS thread. All async
  work happens on background tokio tasks and is bridged to the JS thread via
  `mpsc` channels + thread-local registries — the Phase 3 pattern.
- **`tokio-tungstenite` 0.26 already in the workspace**, already used by the CDP
  server/client (`oxibrowser-cdp`). Same runtime (`current_thread`), same TLS
  path (`MaybeTlsStream`). Reuse it; do not add a second WebSocket library.
- **Phase 3 deadlock lesson (hard-won).** On a `tokio::runtime::Builder::
  new_current_thread()` runtime, blocking inside `rt.block_on(async{...})`
  starves spawned tasks. Polling must be `try_recv()` + `sleep().await`, never
  a blocking `recv()`.
- **Phase 3 idle-condition lesson (applied here).** `settle_to_idle` returns
  only when there is genuinely no outstanding async work. Phase 3 added
  `pending_fetch`; WebSocket adds `pending_ws` (see D5).

## Design decisions

### D1 — Background tokio task per socket

Each `new WebSocket(url)` spawns one tokio task on the existing current-thread
runtime. The task owns the live `WebSocketStream` (split into sink + stream) and
runs for the lifetime of the connection:

- Reads inbound frames from the stream and forwards them to `WS_EVENT_RX`.
- Reads commands from a per-socket `mpsc::Receiver<WsCmd>` (`Send`, `Close`) and
  applies them to the sink.
- On handshake completion → emits `Open`. On read error / peer close / own close
  → emits `Close` (and `Error` on abnormal close) and exits.

The task is selected off the runtime like the Phase 3 fetch tasks; it survives
across pump cycles because the runtime is retained for the session.

### D2 — Thread-local `PENDING_WS` registry (id → WsState)

Mirrors Phase 3's `PENDING_FETCH`. `boa` roots (`JsFunction` callbacks) live
here, held across pump cycles — the same safe pattern as `LISTENER_REGISTRY`
and `PENDING_FETCH`. No `#[derive(Trace, Finalize)]` on the container.

```text
enum WsState {
    Connecting {
        url: String,
        onopen:   Option<JsValue>,   // set before Open arrives (JS assigns eagerly)
        onmessage:Option<JsValue>,
        onclose:  Option<JsValue>,
        onerror:  Option<JsValue>,
        binary_type: BinaryType,     // 'arraybuffer' default
        listeners: HashMap<String, Vec<JsValue>>, // addEventListener
    },
    Open {
        url: String,
        protocol: String,
        extensions: String,
        // same callback fields as Connecting
        binary_type: BinaryType,
        listeners: HashMap<String, Vec<JsValue>>,
    },
    Closed,
}
```

- `PENDING_WS: RefCell<HashMap<u64, WsState>>`
- `NEXT_WS_ID: Cell<u64>`; `next_ws_id()`.
- Callbacks are `Option<JsValue>` and re-fetched at settle time (they may be
  reassigned between pump cycles). `ws.onopen = cb` writes into the registry.
- `addEventListener(type, cb)` appends to `listeners[type]`; both on-properties
  and listeners fire on the corresponding event.

### D3 — Two channels

- `WS_CMD_TX: Sender<WsCmd>` per socket (held in the background task).
  `WsCmd::Send(Text|Binary)`, `WsCmd::Close { code, reason }`.
- `WS_EVENT_RX: RefCell<Option<Receiver<WsEvent>>>` (thread-local, single shared
  receiver installed by a new `SetWsChannel` command, exactly like Phase 3's
  `SetFetchChannel`). `WsEvent` is id-keyed:

```text
enum WsEvent {
    Open   { id, protocol, extensions },
    Message{ id, data: WsData },        // WsData::Text(String) | Binary(Vec<u8>)
    Close  { id, code: u16, reason: String, was_clean: bool },
    Error  { id, message: String },
}
```

Single shared receiver + id routing = one drain site, O(1) dispatch, no
per-request channels (the Phase 3 simplification).

### D4 — Message flow

```
new WebSocket(url):
  1. mint id = next_ws_id()
  2. PENDING_WS[id] = Connecting { on*: None, ... }
  3. JS object created, readyState=0 (CONNECTING), url set
  4. return the object (constructor is synchronous; handshake is async)
  5. background task spawned: connect_async(url) with MaybeTlsStream

  task:
  - connect ok   -> WS_EVENT_RX.send(Open{id, protocol, extensions})
  - connect fail -> WS_EVENT_RX.send(Error{id, ...}) then Close{id, 1006, ""}

ws.send(data):  JS -> per-socket cmd tx -> task sink.send(Text/Binary)
                (no JS-block; bufferedAmount stays 0)

server pushes:  task stream.next() -> WS_EVENT_RX.send(Message{id, data})

ws.close(code?, reason?): JS -> cmd tx Close -> task close handshake
                          -> WS_EVENT_RX.send(Close{...})
                          -> task exits; PENDING_WS[id] = Closed
```

### D5 — Event pumping + the idle condition (the Phase 3 lesson)

**`drain_ws_events(ctx)` runs at the start of `drain_timers`**, alongside
`drain_pending_fetch_responses`. It `try_recv()`s every available `WsEvent`
into a `Vec` first (releasing the `WS_EVENT_RX` borrow), then settles each:
transitions `WsState` (Connecting→Open→Closed), fires on-properties +
listeners, constructs the `MessageEvent`/`CloseEvent`/`Event` objects.

**Idle condition gains `pending_ws`** — this is the gap the design review
caught. A top-level

```js
const ws = new WebSocket(url);
ws.onopen = () => { window.__opened = true; };
```

assigns `onopen` *after* the constructor returns but the handshake is still in
flight. If the pump returned while the socket is CONNECTING, the `Open` event
could arrive with no handler attached, or after eval ended. Therefore:

```text
pending_ws = PENDING_WS has any entry in state Connecting
          OR WS_EVENT_RX has a ready event (try_recv non-empty)
```

`settle_to_idle` (and the post-eval settle pump) returns only when:

```text
pending_timers == 0 && pending_microtasks == 0 && !pending_fetch && !pending_ws
```

While `pending_ws` is true, the pump polls on a short interval (2 ms, capped by
the remaining budget) so background events are delivered to JS. The connect
timeout (D6) guarantees CONNECTING cannot hold the pump forever: a failed/timed-
out connect emits `Error`+`Close`, transitioning the state to `Closed`, which
clears `pending_ws`.

The post-eval settle pump remains gated on `!PENDING_FETCH.is_empty() ||
!PENDING_WS.has_open_work()` so a bare top-level `setInterval(0)` (no async
work) still does not over-fire — the Phase 3 gate generalised.

### D6 — Connect timeout & errors

- `connect_async` wrapped with a **10 s** timeout. Timeout/failure → `Error`
  event (`message` summarising the failure) then `Close` (`{ code: 1006, reason:
  "", was_clean: false }`). `readyState` → CLOSED.
- Abnormal read error (peer reset) → `Error` + `Close(1006, "", false)`.
- Peer-initiated clean close → `Close(reported code, reason, true)`. (We propagate
  the code/reason tungstenite gives us; do not over-engineer exact RFC mapping.)
- `onerror` always fires before `onclose` on failures (matches Chrome).

### D7 — JS API surface (full standard, locked)

| Member | Behavior |
|---|---|
| `new WebSocket(url, protocols?)` | id, CONNECTING, spawn task, return object. `protocols` accepted and sent as `Sec-WebSocket-Protocol`; selection stored in `protocol` on Open. |
| `readyState` | 0 CONNECTING, 1 OPEN, 2 CLOSING, 3 CLOSED |
| `url` | the resolved URL string |
| `protocol` | "" until Open; selected subprotocol (or "") |
| `extensions` | "" (we negotiate none) |
| `binaryType` | `"arraybuffer"` (writable; blob not supported — see Non-goals) |
| `bufferedAmount` | `0` (unbounded send channel; no backpressure) |
| `send(data)` | text → `WsCmd::Send(Text)`; typed array/ArrayBuffer → `Binary`. No-op (or console warn) if not OPEN. |
| `close(code?, reason?)` | CLOSING → `WsCmd::Close` → CLOSED on Close event. |
| `onopen/onmessage/onclose/onerror` | on-properties, assignable any time |
| `addEventListener(type, cb)` / `removeEventListener` | per-type listener vec |

`CONNECTING/OPEN/CLOSING/CLOSED` numeric constants on the constructor (0–3).

## Interfaces (locked)

```rust
// network/ws.rs (new)
pub enum WsCmd { Send(WsData), Close { code: Option<u16>, reason: Option<String> } }
pub enum WsData { Text(String), Binary(Vec<u8>) }

pub enum WsEvent {
    Open    { id: u64, protocol: String, extensions: String },
    Message { id: u64, data: WsData },
    Close   { id: u64, code: u16, reason: String, was_clean: bool },
    Error   { id: u64, message: String },
}

/// Spawn the per-socket background task. Called from the JS-thread bridge.
pub fn spawn_ws_task(
    id: u64,
    url: String,
    protocols: Vec<String>,
    cmd_rx: tokio::sync::mpsc::Receiver<WsCmd>,
    event_tx: tokio::sync::mpsc::Sender<WsEvent>,
);
```

```rust
// runtime.rs (JS-thread side) — new thread-locals + command
pub enum SetWsChannel { /* carries ws_cmd setup + ws_event_rx */ }
thread_local! {
    static PENDING_WS: RefCell<HashMap<u64, WsState>> = const { RefCell::new(HashMap::new()) };
    static NEXT_WS_ID: Cell<u64> = const { Cell::new(1) };
    static WS_EVENT_RX: RefCell<Option<Receiver<WsEvent>>> = const { RefCell::new(None) };
}
fn next_ws_id() -> u64;
fn drain_ws_events(ctx: &mut Context);     // called at start of drain_timers
fn settle_ws_open(...); settle_ws_message(...); settle_ws_close(...); settle_ws_error(...);
fn fire_ws_callbacks(id, event_type, event_obj, ctx);
```

`drain_timers` becomes:
```text
drain_pending_fetch_responses(ctx);
drain_ws_events(ctx);
... existing timer/microtask drain ...
```

`settle_to_idle` idle condition: `pending_timers==0 && pending_microtasks==0
&& !pending_fetch && !pending_ws`.

## Tests (acceptance — TDD)

Integration tests use a **local tokio-tungstenite echo server** bound to
`ws://127.0.0.1:<ephemeral>` (spin up per test, accept, echo frames back, close
cleanly). No external host, no wss in tests (D6 — same framing after handshake).

1. **onopen fires during eval** — `new WebSocket(url); ws.onopen=()=>{__opened=true}`
   → after `evaluate`, `window.__opened === true`. (Exercises D5 idle condition.)
2. **echo round-trip** — `ws.onopen` sends `"ping"`; `ws.onmessage` stores
   `event.data`. After eval, `__got === "ping"`.
3. **readyState transitions** — CONNECTING(0) immediately after `new`; OPEN(1)
   in onopen; CLOSED(3) in onclose. Asserted via stored globals.
4. **binary round-trip** — send `Uint8Array`, receive `ArrayBuffer` in onmessage
   (`binaryType='arraybuffer'`), bytes match.
5. **client-initiated close** — `ws.close(1000, "bye")` → onclose fires with
   `{code:1000, reason:"bye", wasClean:true}`, readyState CLOSED.
6. **connect failure** — bogus port (`ws://127.0.0.1:1`) → onerror then onclose
   with code 1006, readyState CLOSED. Total < 11 s (connect timeout).
7. **multiple concurrent sockets** — two sockets to the same echo server, both
   round-trip independently; events routed to the correct handlers (id routing).
8. **regression** — `cargo test --workspace` stays green; Phase 3 async fetch
   tests unaffected; `setInterval(0)` still fires exactly once (idle gate
   generalised, not loosened).

Unit tests (mocked channels, no server) for: readyState transitions on injected
events, listener-vec fan-out, `pending_ws` idle flag truthiness across states.

## Non-goals

- **Backpressure / `bufferedAmount` truth** — unbounded send channel;
  `bufferedAmount` stays `0`.
- **`binaryType:'blob'`** — `arraybuffer` only. Blob needs URL.createObjectURL
  (separate work). Assigning `'blob'` is accepted but messages still arrive as
  ArrayBuffer (documented).
- **permessage-deflate** — not negotiated.
- **Exact RFC 6455 close-code mapping on peer close** — propagate what
  tokio-tungstenite reports; do not remap.
- **wss in the test suite** — implemented via `MaybeTlsStream`, but self-signed
  cert overhead is out of scope; ws covers the framing path.
- **`WebSocketStream` / backpressure APIs** — standard `WebSocket` only.

## Open / follow-up

- CDP `Network.webSocketFrameCreated/Closed/Error` events (Phase 5 CDP
  completeness) — emit from the background task as a follow-up, not now.
- `URL.createObjectURL` + Blob (enables `binaryType:'blob'`) — separate Phase 4
  item.
