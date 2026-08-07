# Phase 4 — WebSocket Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the standard browser `WebSocket` JS API (full surface, ws+wss) as a non-blocking, event-loop-pumped async resource, mirroring the Phase 3 async-fetch architecture.

**Architecture:** One background tokio task per socket (`tokio-tungstenite`, already in the workspace). The JS thread (`boa`, `!Send`) holds a thread-local `PENDING_WS` registry (id → state+callbacks) and a single shared `WS_EVENT_RX`. A session.rs bridge (`handle_ws_requests`) routes per-socket commands into tokio tasks and collects events back on one shared channel. `drain_ws_events` runs at the start of `drain_timers`; `settle_to_idle`'s idle condition gains `pending_ws`.

**Tech Stack:** `boa_engine` 0.20, `tokio-tungstenite` 0.26 (workspace), `tokio` current-thread runtime, `futures`, std `mpsc` for JS↔bridge, `tokio::mpsc` for bridge↔socket-task commands.

**Spec:** `docs/superpowers/specs/2026-08-08-phase4-websocket.md`

## Global Constraints

- Pure Rust. No Chromium, no V8. JS via `boa_engine`; rendering via Blitz.
- `tokio-tungstenite = { workspace = true }` — the workspace already pins 0.26; the CDP crate uses it. Do NOT add a second WebSocket library.
- **Phase 3 deadlock lesson:** inside the current-thread tokio runtime, never block on `recv()`; use `try_recv()` + `tokio::time::sleep().await` so spawned tasks get polled.
- **Phase 3 idle lesson:** `settle_to_idle` and the post-eval settle pump must keep spinning while any socket is CONNECTING or has undelivered events (`pending_ws`), else a top-level `onopen` assigned after the constructor never fires.
- `boa` GC roots (callback `JsValue`s) live only in the thread-local `PENDING_WS` registry — same safe pattern as `PENDING_FETCH` and `LISTENER_REGISTRY`. No `#[derive(Trace, Finalize)]` on the container.
- Conventional commits, English messages. clippy clean (`cargo clippy --workspace --all-targets -- -D warnings`). `cargo test --workspace` green.

## File Structure

- **Create** `crates/oxibrowser-core/src/network/ws.rs` — `WsCmd`, `WsData`, `WsEvent`, `run_ws_connection` (the per-socket tokio task body).
- **Modify** `crates/oxibrowser-core/src/network.rs:8` — add `pub mod ws;`.
- **Modify** `crates/oxibrowser-core/Cargo.toml` — add `tokio-tungstenite = { workspace = true }`.
- **Modify** `crates/oxibrowser-core/src/js/runtime.rs` — thread-locals (`PENDING_WS`, `NEXT_WS_ID`, `WS_EVENT_RX`, `WS_REQ_TX`), `WsState` enum, `SetWsChannel` command + `set_ws_channel`, the `WebSocket` constructor/send/close/on*-properties/addEventListener, `drain_ws_events` + settle helpers, `drain_timers` call site, `settle_to_idle` idle condition.
- **Modify** `crates/oxibrowser-core/src/session.rs` — `handle_ws_requests` bridge (mirrors `handle_fetch_requests`), WS channel setup + `set_ws_channel` call, per-socket command-channel registry.

---

## Task 1: `network/ws.rs` — types + connection loop + echo integration test

**Files:**
- Create: `crates/oxibrowser-core/src/network/ws.rs`
- Modify: `crates/oxibrowser-core/src/network.rs:8`
- Modify: `crates/oxibrowser-core/Cargo.toml`
- Test: `crates/oxibrowser-core/src/network/ws.rs` (integration test spinning a local echo server)

**Interfaces:**
- Produces: `WsCmd`, `WsData`, `WsEvent`, `pub async fn run_ws_connection(id, url, protocols, cmd_rx: tokio::mpsc::Receiver<WsCmd>, event_tx: std::mpsc::Sender<WsEvent>)`

- [ ] **Step 1: Add the dependency + module declaration**

`crates/oxibrowser-core/Cargo.toml` — under `[dependencies]`, after `boa_gc`:
```toml
tokio-tungstenite = { workspace = true }
```

`crates/oxibrowser-core/src/network.rs` — after line 7 (`pub mod resource;`) insert:
```rust
pub mod ws;
```

- [ ] **Step 2: Write the failing integration test**

Create `crates/oxibrowser-core/src/network/ws.rs`. Put the test in `#[cfg(test)]` at the bottom; the types + `run_ws_connection` go at the top (defined in Step 3). The test spins a **local tokio-tungstenite echo server**, runs `run_ws_connection` as a tokio task, asserts it emits `Open`, echoes a text frame back as `Message`, and emits `Close` on `WsCmd::Close`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite;

    async fn echo_server(port: u16) {
        let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(msg)) = ws.next().await {
            if msg.is_text() || msg.is_binary() {
                ws.send(msg).await.unwrap();
            }
            if msg.is_close() {
                break;
            }
        }
    }

    #[tokio::test]
    async fn run_ws_connection_echo_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // free port for echo_server to rebind

        let (event_tx, event_rx) = std::sync::mpsc::channel::<WsEvent>();
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<WsCmd>(8);
        let url = format!("ws://127.0.0.1:{port}");

        let server = tokio::spawn(echo_server(port));
        let conn = tokio::spawn(run_ws_connection(1, url.clone(), vec![], cmd_rx, event_tx));

        // Open
        let open = event_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(open, WsEvent::Open { id: 1, .. }), "got {open:?}");

        // echo
        cmd_tx.send(WsCmd::Send(WsData::Text("ping".into()))).await.unwrap();
        let msg = event_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(msg, WsEvent::Message { id: 1, data: WsData::Text("ping".into()) });

        // close
        cmd_tx.send(WsCmd::Close { code: Some(1000), reason: Some("bye".into()) }).await.unwrap();
        let close = event_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(close, WsEvent::Close { id: 1, code: 1000, ref reason, was_clean: true } if reason == "bye"), "got {close:?}");

        conn.await.unwrap();
        server.abort();
    }
}
```

- [ ] **Step 3: Run test to verify it fails (compile error)**

Run: `cargo test -p oxibrowser-core --lib network::ws`
Expected: FAIL — `run_ws_connection`, `WsCmd`, `WsData`, `WsEvent` undefined.

- [ ] **Step 4: Write minimal implementation — types + connection loop**

Top of `crates/oxibrowser-core/src/network/ws.rs`:
```rust
//! WebSocket connection task + wire types for the JS WebSocket API.
//!
//! One background tokio task runs `run_ws_connection` per JS `WebSocket`.
//! Commands arrive on a per-socket tokio channel; events flow back on a single
//! shared std channel (id-keyed), drained by `drain_ws_events` on the JS thread.

use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::Message;

/// Outbound command from the session bridge to a socket task.
#[derive(Debug)]
pub enum WsCmd {
    Send(WsData),
    Close { code: Option<u16>, reason: Option<String> },
}

/// Payload for a `Send`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsData {
    Text(String),
    Binary(Vec<u8>),
}

/// Inbound event from a socket task to the JS thread (id-keyed routing).
#[derive(Debug)]
pub enum WsEvent {
    Open { id: u64, protocol: String, extensions: String },
    Message { id: u64, data: WsData },
    Close { id: u64, code: u16, reason: String, was_clean: bool },
    Error { id: u64, message: String },
}

/// The per-socket background task body. Owns the live `WebSocketStream`.
///
/// Emits `Open` on handshake success, `Message` for each inbound text/binary
/// frame, `Close` (with `was_clean`) on a clean shutdown, and `Error`+`Close`
/// (code 1006, `was_clean: false`) on connect failure / timeout / read error.
pub async fn run_ws_connection(
    id: u64,
    url: String,
    protocols: Vec<String>,
    mut cmd_rx: tokio::sync::mpsc::Receiver<WsCmd>,
    event_tx: std::sync::mpsc::Sender<WsEvent>,
) {
    let mut req = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => {
            fail(id, &event_tx, format!("invalid url: {e}"));
            return;
        }
    };
    if !protocols.is_empty() {
        let h = req.headers_mut();
        // join is fine for feature-detect; exact subprotocol negotiation is a non-goal
        let _ = h.insert(
            "sec-websocket-protocol",
            protocols.join(", ").parse().unwrap_or_else(|_| " ".parse().unwrap()),
        );
    }

    let connect = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(req),
    );
    let ws_stream = match connect.await {
        Ok(Ok((ws, resp))) => {
            let protocol = resp
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let _ = event_tx.send(WsEvent::Open {
                id,
                protocol,
                extensions: String::new(),
            });
            ws
        }
        Ok(Err(e)) => {
            fail(id, &event_tx, format!("connect failed: {e}"));
            return;
        }
        Err(_) => {
            fail(id, &event_tx, "connect timeout".into());
            return;
        }
    };

    let (mut sink, mut stream) = ws_stream.split();

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(WsCmd::Send(WsData::Text(t))) => {
                    if sink.send(Message::text(t)).await.is_err() {
                        fail(id, &event_tx, "send failed".into());
                        return;
                    }
                }
                Some(WsCmd::Send(WsData::Binary(b))) => {
                    if sink.send(Message::binary(b)).await.is_err() {
                        fail(id, &event_tx, "send failed".into());
                        return;
                    }
                }
                Some(WsCmd::Close { code, reason }) => {
                    let code = code.unwrap_or(1000);
                    let cf = tungstenite::protocol::CloseFrame {
                        code: CloseCode::from(code),
                        reason: reason.unwrap_or_default().into(),
                    };
                    let _ = sink.send(Message::close_some(cf)).await;
                    let _ = sink.close().await;
                    let _ = event_tx.send(WsEvent::Close {
                        id,
                        code,
                        reason: reason.unwrap_or_default(),
                        was_clean: true,
                    });
                    return;
                }
                None => return,
            },
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let _ = event_tx.send(WsEvent::Message { id, data: WsData::Text(t.into()) });
                }
                Some(Ok(Message::Binary(b))) => {
                    let _ = event_tx.send(WsEvent::Message { id, data: WsData::Binary(b.to_vec()) });
                }
                Some(Ok(Message::Close(c))) => {
                    let (code, reason) = c
                        .map(|cf| (u16::from(cf.code), cf.reason.into_owned()))
                        .unwrap_or((1000, String::new()));
                    let _ = event_tx.send(WsEvent::Close { id, code, reason, was_clean: true });
                    return;
                }
                Some(Ok(_)) => { /* ping/pong ignored */ }
                Some(Err(e)) => {
                    fail(id, &event_tx, format!("read error: {e}"));
                    return;
                }
                None => {
                    let _ = event_tx.send(WsEvent::Close {
                        id, code: 1006, reason: String::new(), was_clean: false,
                    });
                    return;
                }
            }
        }
    }
}

/// Emit `Error` then `Close(1006, "", false)` — the connect-failure path.
fn fail(id: u64, event_tx: &std::sync::mpsc::Sender<WsEvent>, message: String) {
    let _ = event_tx.send(WsEvent::Error { id, message });
    let _ = event_tx.send(WsEvent::Close {
        id,
        code: 1006,
        reason: String::new(),
        was_clean: false,
    });
}
```

> NOTE on `Message::close_some`: `tungstenite` 0.26 may not expose `close_some`. If the compiler rejects it, build the close frame with `Message::close_some` replaced by sending a `Message::Close(Some(cf))` directly, or use `sink.feed(Message::Close(Some(cf))).await.ok(); sink.flush().await.ok();`. Match what the installed `tungstenite` 0.26 API offers; the contract is "send a close frame then a clean Close event".

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p oxibrowser-core --lib network::ws`
Expected: PASS — `run_ws_connection_echo_roundtrip`.

- [ ] **Step 6: clippy clean**

Run: `cargo clippy -p oxibrowser-core --lib -- -D warnings`
Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/oxibrowser-core/src/network/ws.rs crates/oxibrowser-core/src/network.rs crates/oxibrowser-core/Cargo.toml
git commit -m "feat(core): WebSocket connection task + wire types (Phase 4)

run_ws_connection runs as a background tokio task per JS WebSocket:
connect_async (ws/wss via MaybeTlsStream), select on per-socket cmd channel
and inbound stream, emits id-keyed WsEvent (Open/Message/Close/Error) on a
shared std channel. 10s connect timeout. Echo round-trip integration test."
```

---

## Task 2: WS thread-locals + `SetWsChannel` command

**Files:**
- Modify: `crates/oxibrowser-core/src/js/runtime.rs` (near `PENDING_FETCH` at line 94, `SetFetchChannel` at 264, `set_fetch_channel` at 525, the `JsCommand` handler at 1095)

**Interfaces:**
- Consumes: `crate::network::ws::{WsCmd, WsData, WsEvent}` from Task 1.
- Produces: `PENDING_WS`, `NEXT_WS_ID`, `WS_EVENT_RX`, `WS_REQ_TX` thread-locals; `WsState` enum; `SetWsChannel` `JsCommand` variant; `JsRuntime::set_ws_channel(...)`.

- [ ] **Step 1: Write the failing unit test**

In the `#[cfg(test)]` block near the fetch tests (around `runtime.rs:9600`), add:
```rust
#[tokio::test]
async fn test_ws_registry_and_id_minting() {
    let mut rt = JsRuntime::new();
    // No channel installed yet: ids still mint and registry grows.
    let id1 = next_ws_id();
    let id2 = next_ws_id();
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    // Install WS channels and assert they don't panic.
    let (req_tx, req_rx) = std::sync::mpsc::channel::<WsReqMsg>();
    let (ev_tx, ev_rx) = std::sync::mpsc::channel::<WsEvent>();
    rt.set_ws_channel(req_tx, ev_rx);
    // hold the receivers so senders stay valid through the eval
    let _ = (req_rx, ev_tx);
    let r = rt.evaluate("typeof WebSocket").await.unwrap();
    assert!(r.is_ok(), "eval should not panic");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p oxibrowser-core --lib js::runtime::tests::test_ws_registry_and_id_minting`
Expected: FAIL — `next_ws_id`, `WsReqMsg`, `set_ws_channel` undefined.

- [ ] **Step 3: Write the implementation**

**(a) Thread-locals** — insert after the `RESPONSE_RX` block (after `runtime.rs:107`):
```rust
thread_local! {
    /// Per-socket JS state + callbacks, keyed by id. Mirrors `PENDING_FETCH`.
    static PENDING_WS: RefCell<HashMap<u64, WsState>> =
        const { RefCell::new(HashMap::new()) };
    /// Shared WS event receiver (background→JS, id-routed), installed by `SetWsChannel`.
    static WS_EVENT_RX: RefCell<Option<Receiver<WsEvent>>> =
        const { RefCell::new(None) };
    /// Shared WS request sender (JS→bridge): Connect/Send/Close.
    static WS_REQ_TX: RefCell<Option<Sender<WsReqMsg>>> =
        const { RefCell::new(None) };
    static NEXT_WS_ID: Cell<u64> = const { Cell::new(1) };
}

fn next_ws_id() -> u64 {
    NEXT_WS_ID.with(|c| {
        let id = c.get();
        c.set(id.wrapping_add(1));
        id
    })
}

fn ws_has_open_work() -> bool {
    PENDING_WS.with(|m| m.borrow().values().any(|s| !matches!(s, WsState::Closed)))
        || WS_EVENT_RX.with(|cell| {
            cell.borrow().as_ref().map_or(false, |rx| rx.try_recv().is_ok())
        })
}
```
> The `ws_has_open_work()` peek-recv will consume an event; that is only used for the idle check in Task 5 where we drain immediately after. **Simplify in Task 5:** use `pending_ws = any Connecting state` (registry-only), since events are drained every pump — see Task 5 note.

Bring `WsEvent` into scope in the imports near the top of `runtime.rs` (where `FetchResponseMsg` is imported):
```rust
use crate::network::ws::{WsCmd, WsData, WsEvent};
```

**(b) `WsState` + `WsReqMsg`** — place near `PendingFetch`:
```rust
#[derive(Clone)]
enum BinaryType { ArrayBuffer }
impl Default for BinaryType { fn default() -> Self { BinaryType::ArrayBuffer } }

/// JS-side per-socket state. Callbacks are re-read at settle time.
enum WsState {
    Connecting {
        url: String,
        onopen: Option<JsValue>,
        onmessage: Option<JsValue>,
        onclose: Option<JsValue>,
        onerror: Option<JsValue>,
        binary_type: BinaryType,
        listeners: HashMap<String, Vec<JsValue>>,
    },
    Open {
        url: String,
        protocol: String,
        extensions: String,
        onopen: Option<JsValue>,
        onmessage: Option<JsValue>,
        onclose: Option<JsValue>,
        onerror: Option<JsValue>,
        binary_type: BinaryType,
        listeners: HashMap<String, Vec<JsValue>>,
    },
    Closing {
        onclose: Option<JsValue>,
        onerror: Option<JsValue>,
        listeners: HashMap<String, Vec<JsValue>>,
    },
    Closed,
}

/// JS→bridge request, id-keyed.
enum WsReqMsg {
    Connect { id: u64, url: String, protocols: Vec<String> },
    Send { id: u64, data: WsData },
    Close { id: u64, code: Option<u16>, reason: Option<String> },
}
```

**(c) `SetWsChannel` command** — add a variant next to `SetFetchChannel` (`runtime.rs:264`):
```rust
/// Install the WebSocket channels so JS can open realtime connections.
SetWsChannel {
    request_tx: std::sync::mpsc::Sender<WsReqMsg>,
    response_rx: std::sync::mpsc::Receiver<WsEvent>,
    response_tx: Sender<JsResponse>,
},
```

**(d) `set_ws_channel` method** — mirror `set_fetch_channel` (`runtime.rs:525`):
```rust
/// Set the WebSocket channels. Must be called before JS can use `WebSocket`.
pub fn set_ws_channel(
    &mut self,
    request_tx: std::sync::mpsc::Sender<WsReqMsg>,
    event_rx: std::sync::mpsc::Receiver<WsEvent>,
) {
    let (ack_tx, ack_rx) = mpsc::channel::<JsResponse>();
    if let Err(e) = self.cmd_tx.send(JsCommand::SetWsChannel {
        request_tx,
        response_rx: event_rx,
        response_tx: ack_tx,
    }) {
        tracing::error!(error = %e, "failed to send SetWsChannel: JS thread has died");
        return;
    }
    let _ = ack_rx.recv();
}
```

**(e) Command handler** — in the match arm (next to `SetFetchChannel` at `runtime.rs:1095`):
```rust
JsCommand::SetWsChannel {
    request_tx,
    response_rx,
    response_tx,
} => {
    WS_REQ_TX.with(|cell| *cell.borrow_mut() = Some(request_tx));
    WS_EVENT_RX.with(|cell| *cell.borrow_mut() = Some(response_rx));
    let _ = response_tx.send(JsResponse::Ok(Value::undefined()));
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p oxibrowser-core --lib js::runtime::tests::test_ws_registry_and_id_minting`
Expected: PASS.

- [ ] **Step 5: clippy clean + commit**

Run: `cargo clippy -p oxibrowser-core --lib -- -D warnings`
```bash
git add crates/oxibrowser-core/src/js/runtime.rs
git commit -m "feat(core): WebSocket thread-locals + SetWsChannel command (Phase 4)"
```

---

## Task 3: session.rs — WS bridge + channel setup

**Files:**
- Modify: `crates/oxibrowser-core/src/session.rs` (near `handle_fetch_requests` at 111; channel setup at 310)

**Interfaces:**
- Consumes: `WsReqMsg`, `WsEvent`, `WsCmd`, `run_ws_connection` from Tasks 1–2; `JsRuntime::set_ws_channel`.
- Produces: `handle_ws_requests(...)`; WS channels wired in `Session::new`-equivalent.

- [ ] **Step 1: Read the fetch bridge to mirror it**

Read `crates/oxibrowser-core/src/session.rs:111-234` (the Phase 3 `handle_fetch_requests` — the `try_recv()` + `sleep().await` polling pattern) and lines `309-335` (channel creation + `set_fetch_channel` call + spawn). The WS bridge follows the **exact same shape**.

- [ ] **Step 2: Write the bridge**

Insert `handle_ws_requests` next to `handle_fetch_requests`. It polls `ws_req_rx` (std channel) via `try_recv()` + `tokio::time::sleep(1ms).await` inside the same `block_on`, maintaining a `HashMap<u64, tokio::sync::mpsc::Sender<WsCmd>>` registry:

```rust
/// Background WebSocket bridge: routes Connect/Send/Close from the JS thread
/// to per-socket tokio tasks, and forwards nothing back (events go straight
/// to the JS-thread WS_EVENT_RX via the shared event channel).
fn handle_ws_requests(
    ws_req_rx: std::sync::mpsc::Receiver<WsReqMsg>,
    ws_event_tx: std::sync::mpsc::Sender<WsEvent>,
) {
    use crate::network::ws::run_ws_connection;
    let mut sockets: std::collections::HashMap<u64, tokio::sync::mpsc::Sender<WsCmd>> =
        std::collections::HashMap::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("ws bridge runtime");
    rt.block_on(async move {
        loop {
            while let Ok(req) = ws_req_rx.try_recv() {
                match req {
                    WsReqMsg::Connect { id, url, protocols } => {
                        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<WsCmd>(16);
                        let event_tx = ws_event_tx.clone();
                        sockets.insert(id, cmd_tx);
                        tokio::spawn(async move {
                            run_ws_connection(id, url, protocols, cmd_rx, event_tx).await;
                        });
                    }
                    WsReqMsg::Send { id, data } => {
                        if let Some(tx) = sockets.get(&id) {
                            let _ = tx.try_send(WsCmd::Send(data));
                        }
                    }
                    WsReqMsg::Close { id, code, reason } => {
                        if let Some(tx) = sockets.get(&id) {
                            let _ = tx.try_send(WsCmd::Close { code, reason });
                        }
                        sockets.remove(&id);
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });
}
```

Bring `WsReqMsg`, `WsEvent` into the imports at the top of `session.rs` (next to `FetchResponseMsg`):
```rust
use crate::js::runtime::{WsReqMsg};
use crate::network::ws::{WsCmd, WsEvent};
```

- [ ] **Step 3: Wire the channels in Session setup**

In the channel-creation block (after the fetch channels at `session.rs:311`), add:
```rust
let (ws_req_tx, ws_req_rx) = std::sync::mpsc::channel::<WsReqMsg>();
let (ws_event_tx, ws_event_rx) = std::sync::mpsc::channel::<WsEvent>();
js_runtime.set_ws_channel(ws_req_tx, ws_event_rx);
```

Next to the `tokio::task::spawn_blocking(handle_fetch_requests(...))` call, spawn the WS bridge:
```rust
std::thread::spawn(move || handle_ws_requests(ws_req_rx, ws_event_tx));
```
(Use a dedicated OS thread — same as the fetch bridge — so `block_on` doesn't share with the main runtime.)

- [ ] **Step 4: Build (no standalone test yet — covered in Task 6)**

Run: `cargo build -p oxibrowser-core`
Expected: builds clean. If a borrow/move error on `ws_req_rx`/`ws_event_tx`, move them into the thread closure.

- [ ] **Step 5: clippy clean + commit**

Run: `cargo clippy -p oxibrowser-core -- -D warnings`
```bash
git add crates/oxibrowser-core/src/session.rs
git commit -m "feat(core): WebSocket session bridge + channel wiring (Phase 4)"
```

---

## Task 4: WebSocket JS constructor + on*-properties + send/close + addEventListener

**Files:**
- Modify: `crates/oxibrowser-core/src/js/runtime.rs` (in `create_context`/global setup, mirror the XMLHttpRequest constructor pattern)

**Interfaces:**
- Consumes: `PENDING_WS`, `next_ws_id`, `WS_REQ_TX`, `WsReqMsg`, `WsState` from Task 2.
- Produces: a `WebSocket` global constructor usable from JS.

- [ ] **Step 1: Write the failing unit test (mocked — no server)**

Near the WS tests:
```rust
#[tokio::test]
async fn test_websocket_constructor_returns_connecting() {
    let mut rt = JsRuntime::new();
    let (req_tx, req_rx) = std::sync::mpsc::channel::<WsReqMsg>();
    let (ev_tx, ev_rx) = std::sync::mpsc::channel::<WsEvent>();
    rt.set_ws_channel(req_tx, ev_rx);
    let _ = (req_rx, ev_tx);
    let r = rt.evaluate(
        "var ws = new WebSocket('ws://127.0.0.1:1/bogus');\
         globalThis.__url = ws.url;\
         globalThis.__rs = ws.readyState;\
         globalThis.__bt = ws.binaryType;\
         ws.onopen = function(){};\
         globalThis.__hasOpen = (typeof ws.onopen === 'function');"
    ).await.unwrap();
    assert!(r.is_ok(), "{:?}", r.error);
    assert_eq!(js_str(&rt, "__url").await, "ws://127.0.0.1:1/bogus");
    assert_eq!(js_num(&rt, "__rs").await, 0.0); // CONNECTING
    assert_eq!(js_str(&rt, "__bt").await, "arraybuffer");
    assert!(js_bool(&rt, "__hasOpen").await);
}
```
(Use whatever small helpers exist next to the fetch tests for reading globals; if none, assert via `rt.evaluate("__rs").await.unwrap().value`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p oxibrowser-core --lib js::runtime::tests::test_websocket_constructor_returns_connecting`
Expected: FAIL — `WebSocket is not defined`.

- [ ] **Step 3: Implement the constructor + properties**

In the global setup (find the XMLHttpRequest registration block via `grep -n XMLHttpRequest` in `runtime.rs`; register `WebSocket` adjacent to it). Build a `NativeFunction` that:
1. Reads `url` (arg 0) and optional `protocols` (arg 1 — string or array).
2. Mints `id = next_ws_id()`.
3. Inserts `PENDING_WS[id] = WsState::Connecting { url, on*: None, ... }`.
4. Sends `WsReqMsg::Connect { id, url, protocols }` on `WS_REQ_TX` (no-op if uninstalled — `let _ =`).
5. Builds a plain JS object `ws` with own properties: `url`, `readyState: 0`, `protocol: ""`, `extensions: ""`, `binaryType: "arraybuffer"`, `bufferedAmount: 0`, and accessor-backed `onopen/onmessage/onclose/onerror` (or plain props with setters that mirror into `PENDING_WS[id]`).
6. Sets `ws.send`, `ws.close`, `ws.addEventListener`, `ws.removeEventListener` as native functions capturing the id.
7. Returns `ws`.

For `send(data)`: if `data.is_string()` → `WsData::Text`; if typed array/ArrayBuffer → `WsData::Binary(bytes)`. Send `WsReqMsg::Send { id, data }`.
For `close(code?, reason?)`: set `readyState=2`, send `WsReqMsg::Close`.
For the on-property setters: write the `JsValue` into the matching field of `PENDING_WS[id]` (mutate in place; handle the Connecting/Open/Closing variant). If a reader is unfamiliar with boa property setters, use `ObjectBuilder` `.accessor(...)` or define the `on*` as plain writable data properties and additionally store into the registry on assignment by intercepting via a `Proxy` — **prefer plain data properties + a `set_onopen`-style indirection is unnecessary; simply store the callback into the registry when JS assigns it is not possible without a setter.** Concretely: define each `on*` as an accessor whose `set` writes into `PENDING_WS[id]`. See how `onreadystatechange`/`onload` are done for XMLHttpRequest in this file and copy that exact pattern.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p oxibrowser-core --lib js::runtime::tests::test_websocket_constructor_returns_connecting`
Expected: PASS.

- [ ] **Step 5: clippy clean + commit**

```bash
git add crates/oxibrowser-core/src/js/runtime.rs
git commit -m "feat(core): WebSocket JS constructor + properties + send/close (Phase 4)"
```

---

## Task 5: `drain_ws_events` + settle helpers + idle condition (D5 — the core)

**Files:**
- Modify: `crates/oxibrowser-core/src/js/runtime.rs` (near `drain_pending_fetch_responses:1630`, `drain_timers:1672`, `settle_to_idle:1423`, the post-eval settle gate at 938)

**Interfaces:**
- Consumes: `WS_EVENT_RX`, `PENDING_WS`, `WsEvent`, `WsState` from Task 2.
- Produces: `drain_ws_events(ctx)`, settle helpers, the corrected idle condition.

- [ ] **Step 1: Write the failing integration test (echo server — acceptance #1)**

Add a test helper that spins an echo server on an ephemeral port (copy the `echo_server` from Task 1's test into the runtime test module, or expose it). Then:
```rust
#[tokio::test]
async fn test_ws_onopen_fires_during_eval() {
    let port = free_port(); // bind TcpListener 127.0.0.1:0, read port, drop — or keep a helper
    let _server = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(echo_server_blocking(port));
    });
    let mut rt = JsRuntime::new();
    let (req_tx, req_rx) = std::sync::mpsc::channel::<WsReqMsg>();
    let (ev_tx, ev_rx) = std::sync::mpsc::channel::<WsEvent>();
    rt.set_ws_channel(req_tx, ev_rx);
    // bridge
    std::thread::spawn(move || crate::session::handle_ws_requests(req_rx, ev_tx));
    let url = format!("ws://127.0.0.1:{port}");
    let r = rt.evaluate(&format!(
        "var ws = new WebSocket('{url}');\
         ws.onopen = function() {{ globalThis.__opened = true; }};"
    )).await.unwrap();
    assert!(r.is_ok(), "{:?}", r.error);
    let opened = rt.evaluate("__opened === true").await.unwrap();
    assert!(opened.is_ok() && opened.value == Some(serde_json::Value::Bool(true)),
        "onopen did not fire during eval");
}
```
> `echo_server_blocking(port)` runs the Task 1 echo server on a dedicated runtime; expose `run_ws_connection`'s server side via a small `pub(crate)` helper or re-implement inline. `handle_ws_requests` must be `pub(crate)`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p oxibrowser-core --lib js::runtime::tests::test_ws_onopen_fires_during_eval`
Expected: FAIL — `__opened` undefined / onopen never fired (idle condition missing).

- [ ] **Step 3: Implement `drain_ws_events` + settle helpers**

Near `drain_pending_fetch_responses` (`runtime.rs:1630`):
```rust
/// Drain all available WebSocket events and fire the matching JS callbacks.
/// Collects into a Vec first to release the `WS_EVENT_RX` borrow before
/// re-entering boa (settling may issue more JS / WS / fetch).
fn drain_ws_events(ctx: &mut Context) {
    let events: Vec<WsEvent> = WS_EVENT_RX.with(|cell| {
        let mut borrowed = cell.borrow_mut();
        let Some(rx) = borrowed.as_mut() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    });
    for ev in events {
        settle_ws_event(ev, ctx);
    }
}

fn settle_ws_event(ev: WsEvent, ctx: &mut Context) {
    match ev {
        WsEvent::Open { id, protocol, extensions } => settle_ws_open(id, protocol, extensions, ctx),
        WsEvent::Message { id, data } => settle_ws_message(id, data, ctx),
        WsEvent::Close { id, code, reason, was_clean } => settle_ws_close(id, code, reason, was_clean, ctx),
        WsEvent::Error { id, message } => settle_ws_error(id, message, ctx),
    }
}
```
Each `settle_ws_*`:
- Mutates `PENDING_WS[id]` (Connecting→Open on Open; →Closed on Close; unchanged-but-error-fires on Error).
- Builds the event object (`Event` for open/error; `MessageEvent` with `data` for message — text→JsString, binary→`Uint8Array`/`ArrayBuffer`; `CloseEvent` with `code/reason/wasClean` for close).
- Fires both the on-property and the per-type listener vec via a shared `fire_ws_callbacks(id, type, event_obj, ctx)`.

`fire_ws_callbacks` mirrors `fire_callback` from Phase 3 (the `if let Some(v) = cb && let Some(o) = v.as_object()` pattern): read on-property, call if function; then read `listeners[type]`, call each.

- [ ] **Step 4: Wire `drain_ws_events` into `drain_timers`**

At `runtime.rs:1675` (right after `drain_pending_fetch_responses(ctx);`):
```rust
drain_ws_events(ctx);
```

- [ ] **Step 5: Add `pending_ws` to the idle condition**

In `settle_to_idle` (`runtime.rs:1430`) and in `ws_has_open_work()`:
- Define `pending_ws` as **any registry entry not in `Closed` state** (registry-only — events are drained every pump pass, so a non-Closed socket is the durable condition):
```rust
let pending_ws = PENDING_WS.with(|m| m.borrow().values().any(|s| !matches!(s, WsState::Closed)));
```
- The idle return condition becomes:
```rust
if pending_timers == 0 && pending_microtasks == 0 && !pending_fetch && !pending_ws {
    return;
}
```
- Replace the placeholder `ws_has_open_work()` peek-recv (Task 2 Step 3a note) with this registry-only definition.

- [ ] **Step 6: Generalise the post-eval settle gate**

At `runtime.rs:938`, the gate is currently `if PENDING_FETCH.with(|m| !m.borrow().is_empty())`. Extend to also settle when WS work is pending:
```rust
if PENDING_FETCH.with(|m| !m.borrow().is_empty())
    || PENDING_WS.with(|m| m.borrow().values().any(|s| !matches!(s, WsState::Closed)))
{
    settle_to_idle(&mut ctx, &job_queue, start, Duration::from_millis(timeout));
}
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo test -p oxibrowser-core --lib js::runtime::tests::test_ws_onopen_fires_during_eval`
Expected: PASS.

- [ ] **Step 8: clippy clean + commit**

```bash
git add crates/oxibrowser-core/src/js/runtime.rs
git commit -m "feat(core): drain_ws_events + pending_ws idle condition (Phase 4)

drain_ws_events runs at the start of drain_timers (alongside the Phase 3 fetch
drain). settle_to_idle's idle condition gains pending_ws (any non-Closed
socket), so a top-level onopen assigned after the constructor still fires.
Post-eval settle gate generalised to WS work."
```

---

## Task 6: Full integration acceptance tests (echo server)

**Files:**
- Modify: `crates/oxibrowser-core/src/js/runtime.rs` (test module)

**Interfaces:** consumes Tasks 1–5.

Implement each acceptance test from the spec, TDD-style (test first, watch it pass — the implementation already exists, so this is verification). Spin one echo server per test (or a shared helper with a fresh port).

- [ ] **Step 1:** `test_ws_echo_roundtrip` — onopen sends `"ping"`, onmessage stores `event.data`; assert `__got === "ping"`. (acceptance #2)
- [ ] **Step 2:** `test_ws_readystate_transitions` — store `readyState` at construction (0), in onopen (1), in onclose (3); assert the sequence. (acceptance #3)
- [ ] **Step 3:** `test_ws_binary_roundtrip` — send `Uint8Array([1,2,3])`, assert onmessage receives an ArrayBuffer with the same bytes (`binaryType='arraybuffer'`). (acceptance #4)
- [ ] **Step 4:** `test_ws_client_close` — `ws.close(1000,"bye")`, assert onclose `{code:1000, reason:"bye", wasClean:true}`, readyState 3. (acceptance #5)
- [ ] **Step 5:** `test_ws_connect_failure` — bogus `ws://127.0.0.1:1`, assert onerror then onclose with code 1006, readyState 3, total < 11s. (acceptance #6)
- [ ] **Step 6:** `test_ws_concurrent_sockets` — two sockets to the same echo server, both round-trip, events routed to correct handlers (store per-socket `__gotA`/`__gotB`). (acceptance #7)
- [ ] **Step 7: Run the whole suite**

Run: `cargo test -p oxibrowser-core --lib js::runtime::tests::test_ws`
Expected: all WS tests PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/oxibrowser-core/src/js/runtime.rs
git commit -m "test(core): WebSocket integration acceptance suite (Phase 4)"
```

---

## Task 7: Regression — full workspace green + setInterval gate intact

**Files:** none (verification only).

- [ ] **Step 1: Full workspace test**

Run: `cargo test --workspace`
Expected: all pass. Pay attention to:
- `test_set_interval_executes_once` (the Phase 3 gate — must still fire exactly once).
- Phase 3 async fetch tests (`test_async_fetch_*`, `test_async_xhr_*`) — unaffected.

- [ ] **Step 2: clippy across the workspace**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Update CHANGELOG**

In `CHANGELOG.md` under `## [Unreleased]` → `### Added`, add:
```markdown
- **`WebSocket` (Phase 4)** — standard browser WebSocket API (full surface, ws+wss). One background tokio task per socket (tokio-tungstenite); events pump on the JS event loop via `drain_ws_events` (alongside the Phase 3 fetch drain). `settle_to_idle` keeps spinning while any socket is CONNECTING, so a top-level `onopen` assigned after the constructor still fires. Local echo-server integration tests cover open/echo/readyState/binary/close/failure/concurrent.
```

- [ ] **Step 4: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): WebSocket (Phase 4) under [Unreleased]"
```

---

## Notes for implementers

- **boa property accessors:** the `on*` properties must be accessors whose `set` writes into `PENDING_WS[id]`, because callbacks assigned in JS after construction must reach the settle-time reader. Copy the exact pattern XMLHttpRequest uses for `onload`/`onreadystatechange` in this file (`grep -n "onreadystatechange" runtime.rs`).
- **GC safety:** `PENDING_WS` holds `JsValue` callbacks in a `RefCell<HashMap>`, same as `PENDING_FETCH` and `LISTENER_REGISTRY`. Do not add `Trace`/`Finalize` to `WsState`; the container is a thread-local.
- **`Message::close_some`:** if `tungstenite` 0.26 lacks it, send `Message::Close(Some(cf))` via `sink.feed(...)` + `sink.flush()`. The contract is "clean close event with the given code/reason".
- **Don't over-engineer close codes.** Propagate what tungstenite reports; exact RFC 6455 mapping is a non-goal.
