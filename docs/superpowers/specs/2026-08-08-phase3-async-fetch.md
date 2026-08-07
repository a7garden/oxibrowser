# Phase 3 Spec — Async (Non-Blocking) fetch / XHR

> Roadmap: `docs/superpowers/specs/2026-08-07-chrome-parity-roadmap.md` (Phase 3).
> Builds on Phase 1 (nav script execution + bootstrap pump) and Phase 2 (live-DOM
> `wait_for`, in-flight counter). Hard constraint: pure Rust (`boa_engine` 0.20 + Blitz).
> No new C deps.

## Goal

`fetch()` and `XMLHttpRequest.send()` **return immediately** and never block the JS
thread. Concurrent in-flight requests run in parallel. Their results resolve on the
event loop: when a response arrives, the JS thread settles the pending `Promise` (fetch)
or fires the XHR callbacks (`onreadystatechange`/`onload`/`onerror`) on the next pump.
A script that issues three slow fetches back-to-back finishes in ≈ the **slowest** one,
not the sum.

## Problem (evidence)

Today the JS thread **synchronously blocks** on every network round-trip it issues:

- `fetch()` closure — `runtime.rs:1709`: `let response = response_rx.recv();` with the
  comment `// TODO(#async-fetch): This blocking recv() holds the JS thread`.
- `xhr.send()` closure — `runtime.rs:1981`: `match response_rx.recv() {`.
- The background `handle_fetch_requests` (`session.rs:108`) is itself serialized:
  `rt.block_on` loops over `fetch_rx.try_recv()` (10 ms sleep) and **awaits each fetch
  to completion before dequeuing the next** (`session.rs:152`). So even requests the JS
  thread fired back-to-back are served one-at-a-time. The in-flight counter (`in_flight`)
  therefore never exceeds 1 for JS-issued fetches, and `click_and_stabilize`'s
  NetworkIdle detection is starved.

Consequences: (a) the JS thread is frozen for the duration of each request — timers,
event handlers, other scripts cannot run; (b) SPA bootstrap latency = Σ round-trips; (c)
a `fetch` inside a `click` handler delays every subsequent automation step by the full
RTT. This is the single largest remaining SPA bottleneck.

## Design

### D1 — Request identity: monotonically-id'd requests on one shared response channel

Replace the **per-request response channel** (`FetchRequestMsg.response_tx` /
`FetchResponseMsg` returned via a fresh `mpsc` per call) with a **single shared response
channel** owned by the JS thread, keyed by a request `id`.

```rust
// runtime.rs
pub struct FetchRequestMsg {
    pub id: u64,            // NEW — routes the response back to its pending slot
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    // response_tx REMOVED
}

pub struct FetchResponseMsg {
    pub id: u64,            // NEW — echoed back from the background thread
    pub status: u16,
    pub status_text: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub error: Option<String>,
}
```

- `id` is minted on the JS thread by a thread-local `Cell<u64>` (starts at 1, never 0).
  JS-thread-only ⇒ no `AtomicU64`, no cross-thread contention.
- The background thread no longer holds a per-request `response_tx`; it pushes every
  `FetchResponseMsg { id, .. }` onto **one** `std::sync::mpsc::Sender<FetchResponseMsg>`.
- The matching `Receiver` lives on the JS thread (held in the `js_thread_loop` stack /
  a thread-local) and is drained by the event-loop pump (D4). `try_recv`, never `recv`.

### D2 — Concurrent background I/O (spawn-per-request)

`handle_fetch_requests` becomes a true dispatcher: on each `fetch_rx.recv()` (blocking
is fine here — this thread does nothing but dispatch), it `tokio::spawn`s an independent
task per request and immediately loops back to accept the next. Each spawned task does
its own `http_client.fetch(&url).await`, then sends the single `FetchResponseMsg { id, .. }`
on the shared response channel and decrements `in_flight`. This removes the serialization
that capped parallelism at 1.

```text
fetch_rx.recv() ──► in_flight +1 ──► tokio::spawn(async {
│                                       client.fetch(url).await;
│                                       response_tx.send(FetchResponseMsg{id,..});
│                                       in_flight -1;
│                                   });
└── loop (accept next request immediately)
```

The shared `response_tx` is cloned into each spawned task (`mpsc::Sender` is `Clone`).
The `10 ms` `try_recv` busy-poll is replaced by a blocking `recv()` (the dispatcher has
no other work). The `in_flight` accounting is unchanged in spirit: +1 before spawn, −1
exactly once per terminal branch inside the spawned task.

### D3 — Pending-resolver registry (thread-local, JS thread)

A thread-local map holds everything needed to settle an in-flight request when its
response arrives. Because `ResolvingFunctions` (and the XHR callback `JsObject`s) are
GC-managed `!Send` values, this registry lives **only** on the JS thread — exactly where
the pump drains responses.

```rust
use boa_engine::object::builtins::JsPromise;
use boa_gc::GcRefCell; // boa 0.20's internal-use RefCell; or std RefCell behind a wrapper

enum PendingFetch {
    Fetch {
        resolvers: ResolvingFunctions, // resolve()/reject() settle the Promise
    },
    Xhr {
        ready_state: Arc<RwLock<f64>>,
        status:    Arc<RwLock<f64>>,
        resp_body: Arc<RwLock<String>>,
        resp_hdrs: Arc<RwLock<String>>,
        onload:    Arc<RwLock<Option<JsValue>>>,
        onerror:   Arc<RwLock<Option<JsValue>>>,
        onrsc:     Arc<RwLock<Option<JsValue>>>,  // onreadystatechange
    },
}

thread_local! {
    static PENDING_FETCH: std::cell::RefCell<std::collections::HashMap<u64, PendingFetch>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    static NEXT_FETCH_ID: std::cell::Cell<u64> = std::cell::Cell::new(1);
}
```

- Insert on dispatch (`fetch`/`xhr.send`); remove-and-settle in the pump (D4).
- `JsValue`/`ResolvingFunctions` stored by value (they are `Trace + Finalize` clones
  rooted by the registry cell while present). They are dropped when the entry is removed
  after settlement, releasing GC roots.

> **Note on GC rooting:** storing boa GC objects in a `thread_local!` `RefCell<HashMap>`
> is the same rooting strategy the existing `LISTENERS`/`__ready` registries already use
> in `runtime.rs:59-70`. boa's `Trace`/`Finalize` derive on the container is required so
> the GC sees the roots; `ResolvingFunctions` and `JsValue` already derive both. If the
> derive cannot be placed on a local enum directly, wrap it in a `#[derive(Trace, Finalize)]`
> struct mirroring the existing registry pattern.

### D4 — fetch closure: return a pending Promise, dispatch, never block

The `fetch_fn` closure (`runtime.rs:1598`) is rewritten:

1. Mint `id = NEXT_FETCH_ID.get()` / increment.
2. `let (promise, resolvers) = JsPromise::new_pending(ctx);`
3. `PENDING_FETCH.with(|m| m.borrow_mut().insert(id, PendingFetch::Fetch { resolvers }));`
4. Build `FetchRequestMsg { id, url, method, headers, body }` (no `response_tx`).
5. `fetch_tx.send(request)`. On send error, remove the pending entry and **reject** the
   promise via `resolvers.reject.call(...)` (no `ctx.eval` of a reject snippet — settle
   the real pending Promise so `.catch` chains fire through the loop).
6. `return Ok(promise.into())` — control returns to JS **immediately**.

The response-object construction (status/headers/text/json/arrayBuffer methods) currently
inline in the closure (lines 1717–1839) moves into the **pump** (D6), where it is built
once and passed to `resolvers.resolve.call(&[response_obj], ctx)`.

### D5 — xhr.send: non-blocking async mode

`xhr.send()` (`runtime.rs:1958`) is rewritten for `async === true` (the default; the code
path that today blocks). The closure:

1. Sets `readyState = 2` (HEADERS_RECEIVED), fires `onreadystatechange` synchronously.
2. Mints `id`, inserts `PendingFetch::Xhr { …shared state cells, callbacks… }`.
3. `fetch_tx.send(FetchRequestMsg { id, url, method, headers: vec![], body })`.
4. Returns `Ok(undefined)` immediately — does **not** wait.

Synchronous XHR (`async === false`) remains blocking on the JS thread (matches Chrome
semantics and is rare in SPAs). It can keep a dedicated one-shot channel for that single
case, or simply use the shared path with an inline `recv()` — a documented carve-out.

### D6 — Event-loop pump: drain pending responses and settle

A new helper drains all currently-available responses and settles their pending entries:

```rust
fn drain_pending_fetch_responses(ctx: &mut Context) {
    RESPONSE_RX.with(|cell| {
        let Some(rx) = &*cell.borrow() else { return };
        while let Ok(resp) = rx.try_recv() {
            let entry = PENDING_FETCH.with(|m| m.borrow_mut().remove(&resp.id));
            match entry {
                Some(PendingFetch::Fetch { resolvers }) => settle_fetch(resolvers, resp, ctx),
                Some(PendingFetch::Xhr { .. })       => settle_xhr(resp, ctx),
                None => { /* stale/dup — ignore */ }
            }
        }
    });
}
```

`settle_fetch` builds the Response object (moved from the old inline code) and calls
`resolvers.resolve.call(&JsValue::undefined(), &[response_obj], ctx)`, or
`resolvers.reject.call(&[JsError::from(...)], ctx)` on error. `settle_xhr` mutates the
shared state cells to `readyState 3 → 4`, sets status/body/headers, then fires
`onreadystatechange`/`onload` (or `onerror`) by calling the stored callbacks — reusing
the exact callback-firing logic the current synchronous XHR closure uses inline.

This helper is called from **every** place the loop already pumps:

- `drain_timers` start (timers may fire `fetch`).
- `settle_to_idle` each pass, before `run_jobs` — so nav-script fetches resolve during
  the bootstrap pump (Phase 1's loop).
- `run_navigation_scripts` after each script eval (a script may `fetch` then `await`).
- the `evaluate` handler (between script eval and the existing microtask/timer drain at
  `runtime.rs:874-877`) so a top-level `await fetch(...)` in an agent `evaluate` settles.

Because settling enqueues microtasks (the resolve job) and (for XHR) calls JS callbacks
which may enqueue more, `drain_pending_fetch_responses` is followed by `ctx.run_jobs()` +
`drain_timers` at each call site — already the existing pattern.

### D7 — Wiring the shared response channel into the JS thread

`SetFetchChannel` currently carries only the request `tx`. Extend it (or add a sibling
field) to also deliver the shared `mpsc::Receiver<FetchResponseMsg>`:

```rust
JsCommand::SetFetchChannel {
    request_tx: std::sync::mpsc::Sender<FetchRequestMsg>,
    response_rx: std::sync::mpsc::Receiver<FetchResponseMsg>, // NEW
    response_tx: Sender<JsResponse>, // ack
}
```

The JS-thread handler stores `response_rx` into the `RESPONSE_RX` thread-local (a
`RefCell<Option<Receiver>>`). `Session::new` (`session.rs:286-310`) creates the single
shared `(response_tx, response_rx)` pair, hands `response_tx` to `handle_fetch_requests`
(cloned per spawned task in D2), and passes `response_rx` through `set_fetch_channel`.

`JsRuntime::set_fetch_channel` signature changes to accept the `Receiver` and forward it.

## Interfaces (locked)

- `FetchRequestMsg { id, url, method, headers, body }` — `response_tx` removed, `id` added.
- `FetchResponseMsg { id, status, status_text, url, headers, body, error }` — `id` added.
- `JsCommand::SetFetchChannel { request_tx, response_rx, response_tx }`.
- `JsRuntime::set_fetch_channel(&mut self, request_tx, response_rx)`.
- `enum PendingFetch { Fetch { resolvers }, Xhr { …shared cells, callbacks… } }` +
  `PENDING_FETCH`, `NEXT_FETCH_ID`, `RESPONSE_RX` thread-locals (JS-thread only).
- `fn drain_pending_fetch_responses(ctx: &mut Context)` — called from the pump sites in D6.
- `handle_fetch_requests` rewritten as a spawn-per-request dispatcher
  (`session.rs:108`), consuming a shared `Sender<FetchResponseMsg>`.

## Tests (acceptance)

All run against an in-process mock HTTP server (the existing test pattern; see the
`#[ignore]` real-site tests for the live equivalents).

1. **Concurrent fetch is parallel (not serial).** Mock two endpoints with deliberate,
   measured delays (e.g. `/slow1` = 300 ms, `/slow2` = 300 ms). A script fires both
   `fetch()` back-to-back and awaits both. Total wall time on the JS thread < 300 ms +
  ε (the sum would be ≈ 600 ms). **Fails on `main`** (serialized ⇒ ≈ 600 ms).
2. **fetch does not block the JS thread.** A script sets `window.__t0 = Date.now()`,
   calls `fetch('/slow')` (300 ms) **without** awaiting, then immediately sets
   `window.__t1 = Date.now()`. After the pump settles, `window.__t1 - window.__t0 < 50 ms`.
   **Fails on `main`** (`recv()` blocks ≈ 300 ms).
3. **fetch resolves on the event loop.** `fetch('/json').then(r => r.json()).then(o =>
   { window.__done = o.value })`. After `evaluate` returns and the pump settles,
   `window.__done === 42`. **Fails on `main`** only if the resolve path is broken; the
   key assertion is that `.then` chains fire **without** a second explicit `evaluate`.
4. **Async XHR is non-blocking.** `xhr.open('GET','/slow',true); xhr.onload=()=>{window.__xhr=1};
   xhr.send(); window.__sent=1;`. After settle, `window.__sent === 1` was set **before**
   the response arrived (assert ordering: `__sent` set synchronously, `__xhr` set later),
   and `window.__xhr === 1` after the pump. **Fails on `main`** (send blocks).
5. **Mixed fetch + timer interleave.** A script calls `fetch('/a')`, then
   `setTimeout(()=>{window.__to=1},50)`. After settle, **both** `window.__fetchA` and
   `window.__to` are set — proving the JS thread kept ticking timers while the fetch was
   in flight. **Fails on `main`** (fetch freezes the thread, timer overdue-or-deadlocked).
6. **Error path rejects.** `fetch('/500')` against an endpoint returning 500 (or a network
   error) rejects; `.catch(e => { window.__err = 1 })` fires. After settle,
   `window.__err === 1`.
7. **Regression:** existing fetch-with-mock-channel and XHR unit tests still pass (the
   public JS contract — `Response.text()/json()`, `xhr.status/responseText`,
   `readyState` transitions — is unchanged; only timing/blocking changes).

## Non-goals (Phase 3)

- **Synchronous XHR deprecation** — `async:false` stays (blocking), documented carve-out.
- **Request prioritization / scheduling** — FIFO dispatch is fine.
- **AbortController cancellation of in-flight fetch** — Phase 4.
- **CORS / preflight / cookie correctness on JS fetch** — Phase 6; Phase 3 reuses the
  existing permissive transport.
- **`ReadableStream` response bodies / streaming** — Phase 6 (streaming bodies); Phase 3
  buffers the full body as today.
- **CDP `Network.*` event emission for JS fetches** — Phase 5; Phase 3 keeps the current
  (silent) behavior, relying on the `in_flight` counter for NetworkIdle.

## Implementation notes

- boa 0.20 has **no** `AsyncResolver`; use `JsPromise::new_pending(ctx)` →
  `(JsPromise, ResolvingFunctions)` and store the `ResolvingFunctions` (two `JsFunction`s)
  in the pending registry. `ResolvingFunctions` derives `Trace + Finalize` (jspromise.rs:100).
- The `response_rx` is a `std::sync::mpsc::Receiver` — `!Send` in name only; it is moved
  onto the JS thread once and never shared, so the `!Send` bound is respected. (If the
  compiler complains about moving it through the channel payload, wrap the handoff in a
  one-shot ack: `Session` sends the command, the JS thread stores the receiver, acks Done.)
- The GC-rooting registry must `#[derive(Trace, Finalize)]` or reuse the existing
  `LISTENERS` thread-local pattern; `boa_gc::GcRefCell` may be needed for the map if the
  bare `std::cell::RefCell` triggers the `Trace` requirement on `thread_local!` contents.
