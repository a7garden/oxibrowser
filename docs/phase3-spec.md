# Phase 3: CDP Events + Network Interception + E2E Tests

## Objective

Make OxiBrowser a real CDP server that Puppeteer/Playwright-compatible clients can
actually use, with event publishing, network interception, and a pure-Rust E2E test
suite that verifies the full stack.

## Architecture

```
CDP Client (Rust test / Puppeteer / Playwright)
         │ CDP WebSocket
         ▼
┌─── CdpServer ──────────────────────────────────┐
│  HTTP: /json/version, /json                     │
│  WS upgrade → spawns CdpSession per connection  │
└─────────────────────────────────────────────────┘
         │
         ▼
┌─── CdpSession ─────────────────────────────────┐
│  NEW: EventBroadcaster (mpsc channel)           │
│  - domain handlers call broadcaster.send_event()│
│  - background task forwards events to WS sink   │
│  - events: Page.loadEventFired, frameNavigated, │
│    Runtime.executionContextCreated,             │
│    Network.requestWillBeSent, Fetch.requestPaused│
└─────────────────────────────────────────────────┘
         │
         ▼
┌─── Core (Browser → Session → Page → Frame) ────┐
│  Session::navigate() emits navigation lifecycle  │
│  hooks that CdpSession translates to CDP events  │
└─────────────────────────────────────────────────┘
```

## Tasks

### T1: CDP Event Broadcasting System
- **Files**: `crates/oxibrowser-cdp/src/event.rs` (NEW), `crates/oxibrowser-cdp/src/session.rs`
- **What**: Create `EventBroadcaster` with `tokio::sync::mpsc` channel. CdpSession
  runs a background task that reads events from the channel and sends them as
  CDP JSON over WebSocket. Domain handlers get a clone of the sender.
- **Verify**: Unit test: send event through broadcaster, receive JSON string.

### T2: Navigation Lifecycle Events
- **Files**: `crates/oxibrowser-cdp/src/session.rs`, `crates/oxibrowser-cdp/src/domains/page.rs`
- **Depends**: T1
- **What**: After `Page.navigate` completes, emit CDP events:
  - `Page.frameNavigated` — frame info with URL, loaderId
  - `Page.loadEventFired` — timestamp
  - `Page.domContentLoadedEventFired` — timestamp
  After `Page.enable`, emit `Page.frameAttached` / `Page.frameDetached` stubs.
- **Verify**: E2E test: send Page.enable + Page.navigate, receive events.

### T3: Runtime Events
- **Files**: `crates/oxibrowser-cdp/src/domains/runtime.rs`
- **Depends**: T1
- **What**: After `Runtime.enable`, emit `Runtime.executionContextCreated` event
  with context info. After `Runtime.evaluate`, emit `Runtime.consoleAPICalled`
  if console.log is detected.
- **Verify**: E2E test: Runtime.enable → receive executionContextCreated.

### T4: Network Domain Events
- **Files**: `crates/oxibrowser-cdp/src/domains/network.rs`
- **Depends**: T1
- **What**: Implement real `Network.enable`/`Network.disable`. When network is
  enabled and a navigation happens, emit:
  - `Network.requestWillBeSent` — request details (URL, method, headers)
  - `Network.responseReceived` — response details (status, mimeType)
  - `Network.loadingFinished` — request loaded
- **Verify**: E2E test: Network.enable + navigate → receive request/response events.

### T5: Fetch Domain (Network Interception)
- **Files**: `crates/oxibrowser-cdp/src/domains/fetch.rs` (NEW)
- **Depends**: T1
- **What**: Implement `Fetch.enable`, `Fetch.disable`, `Fetch.continueRequest`,
  `Fetch.failRequest`, `Fetch.fulfillRequest`. When enabled, outgoing HTTP
  requests are paused and a `Fetch.requestPaused` event is sent to the client.
  Client responds with continue/fail/fulfill.
- **Verify**: Unit test: Fetch.enable → intercept → continue.

### T6: CDP Session Refactor (Dispatch Context)
- **Files**: `crates/oxibrowser-cdp/src/session.rs`, `crates/oxibrowser-cdp/src/domains/mod.rs`
- **Depends**: T1
- **What**: Refactor `dispatch()` to take a `DispatchContext` struct instead of
  individual params. `DispatchContext` holds session + event sender + enabled
  domain state. This lets domain handlers emit events.
- **Verify**: Build passes, existing tests still pass.

### T7: E2E Test Infrastructure
- **Files**: `crates/oxibrowser-cdp/tests/e2e.rs` (NEW)
- **Depends**: T2, T3, T4
- **What**: Create a pure Rust E2E test harness:
  1. Start CdpServer on random port
  2. Connect via tokio-tungstenite as a CDP client
  3. Send CDP commands, read responses and events
  4. Test scenarios:
     - Connect → /json/version → WS upgrade
     - Page.enable + Page.navigate → receive loadEventFired
     - Runtime.enable → receive executionContextCreated
     - DOM.getDocument → verify node tree
     - Network.enable + navigate → receive request/response events
- **Verify**: `cargo test --workspace` passes with all E2E tests.

## Execution Batches

```
Batch 1 (parallel): [T1] — Event broadcasting (foundation)
Batch 2 (parallel): [T2, T3, T4, T5, T6] — All domain work, depends on T1
Batch 3 (sequential): [T7] — E2E tests, depends on T2-T6
```

## Acceptance Criteria

1. `cargo build --workspace` → 0 errors, 0 warnings
2. `cargo test --workspace` → all tests pass
3. CDP events are emitted on navigation (Page.loadEventFired, frameNavigated)
4. Network domain emits request/response lifecycle events
5. Fetch domain can intercept, pause, and continue requests
6. E2E tests verify full CDP flow: connect → navigate → events → DOM queries
7. Zero non-Rust dependencies (no Node.js, no Puppeteer, no Playwright)
