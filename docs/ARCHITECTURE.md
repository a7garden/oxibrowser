# OxiBrowser Architecture

## Overview

OxiBrowser is a headless browser engine built in pure Rust. It follows the
[Lightpanda](https://github.com/lightpanda-io/browser) architecture pattern
(Browser → Session → Page → Frame) but is fully Rust-native with no C/C++
dependencies.

## Core Hierarchy

```
Browser
 └── Session
      └── Page
           └── Frame (root)
                └── Frame (iframe)
```

| Level | Type | Responsibility |
|-------|------|---------------|
| `Browser` | Top-level singleton | Owns sessions, HTTP client, global cookie jar, browser config |
| `Session` | Browsing context group | Owns pages, navigation history, local storage, JS runtime |
| `Page` | Loaded document | Owns root frame, sub-resources, metadata (status, content-type, title) |
| `Frame` | Document frame | Owns parsed DOM (`Document`), child frames (iframes), raw HTML |

Each level has a unique atomic ID (`BrowserId`, `SessionId`, `PageId`, `FrameId`, `NodeId`)
with thread-safe counters (`AtomicU64` / `AtomicU32`).

## Crate Layout

```
oxibrowser/
├── crates/
│   ├── oxibrowser/          # Binary + CLI (4,242 lines)
│   │   └── src/
│   │       ├── main.rs      # CLI: fetch, extract, run, session, serve, describe, skill, version
│   │       ├── output.rs    # CliResponse JSON wrapper, truncation, field filtering
│   │       ├── validate.rs  # Input validation (URL, selectors, control chars)
│   │       ├── describe.rs  # CLI schema introspection (for agents)
│   │       ├── skill.rs     # Agent skill guide markdown
│   │       ├── session/     # Session REPL (stdin/stdout JSON)
│   │       │   ├── mod.rs          # Event loop, signal handling
│   │       │   ├── parser.rs       # 22 command parser (30 tests)
│   │       │   ├── executor.rs     # Command → Tab method → CliResponse
│   │       │   └── tab_manager.rs  # Multi-tab management
│   │       └── lib.rs
│   ├── oxibrowser-core/     # Core engine (19,794 lines)
│   │   └── src/
│   │       ├── browser.rs   # Browser singleton
│   │       ├── session.rs   # Session: navigate, evaluate, apply_mutations
│   │       ├── page.rs      # Page: metadata, screenshots
│   │       ├── frame.rs     # Frame: DOM holder, title extraction
│   │       ├── config.rs    # BrowserConfig
│   │       ├── encoding.rs  # Charset detection (encoding_rs)
│   │       ├── error.rs     # CoreError
│   │       ├── js/
│   │       │   ├── runtime.rs        # JsRuntime — boa_engine (6,400+ lines)
│   │       │   ├── dom_snapshot.rs   # DomSnapshot + DomMutation
│   │       │   ├── input.rs          # JS key/mouse/insert dispatch
│   │       │   └── job_queue.rs      # TokioJobQueue for timers
│   │       ├── css/
│   │       │   ├── render.rs         # ASCII/Unicode text renderer
│   │       │   ├── screenshot.rs     # PNG renderer + bitmap font
│   │       │   └── font_8x16.bin     # 8×16 bitmap font (1520 bytes)
│   │       └── network/
│   │           ├── client.rs         # HttpClient + intercept
│   │           ├── cookie.rs         # CookieJar
│   │           ├── resource.rs       # Resource tracking
│   │           ├── ip_filter.rs      # SSRF CIDR blocking
│   │           ├── robots.rs         # RFC 9309 robots.txt
│   │           └── intercept.rs      # PausedRequestRegistry
│   ├── oxibrowser-cdp/     # CDP server (4,583 lines)
│   │   └── src/
│   │       ├── server.rs             # HTTP + WebSocket server
│   │       ├── session.rs            # CDP session management
│   │       ├── protocol.rs           # CdpRequest/CdpResponse/CdpEvent
│   │       ├── event.rs              # Event broadcasting
│   │       └── domains/
│   │           ├── browser.rs        # Browser domain
│   │           ├── dom.rs            # DOM domain
│   │           ├── fetch.rs          # Fetch domain (interception)
│   │           ├── input.rs          # Input domain (key/mouse)
│   │           ├── network.rs        # Network domain
│   │           ├── oxi.rs            # OXI domain (AI extensions)
│   │           ├── page.rs           # Page domain (navigate, screenshot)
│   │           ├── runtime.rs        # Runtime domain (evaluate)
│   │           └── target.rs         # Target domain
│   └── oxibrowser-webapi/  # DOM (1,587 lines)
│       └── src/dom/
│           ├── document.rs           # Document: parse, query, mutate
│           ├── node.rs               # Node, NodeId, NodeType
│           └── tree.rs               # Tree: adjacency list, DFS, BFS
```

## Data Flow

### Page Load

```
Client → CDP Server → Session::navigate()
                         │
                         ▼
                    HttpClient::fetch()
                         │
                         ▼
                    Frame::from_html()
                         │
                         ▼
                    Document::parse()
                         │
                         ▼
                    DomSnapshot::from_frame()
                         │
                         ▼
                    JsRuntime::set_dom_snapshot()
```

### JavaScript Evaluation

```
Client → CDP Server → Session::evaluate("document.querySelector('h1').textContent")
                         │
                         ▼
                    JsRuntime::evaluate()
                         │
                         ▼
                    mpsc::Sender → JS Thread
                                        │
                                        ▼
                                   boa_engine::Context::eval()
                                        │
                                        ├── document.querySelector → DomSnapshot lookup
                                        ├── element.textContent → DomSnapshot read
                                        └── element.textContent = 'new' → DomMutation write
                                        │
                                        ▼
                                   mpsc::Receiver ← JS Thread
                         │
                         ▼
                    Session::apply_mutations()
                         │
                         ├── SetAttribute → Frame::set_attribute()
                         ├── CreateElement → Document::create_element_node()
                         ├── AppendChild → Tree::append_child()
                         └── SetTextContent → Frame::set_text_content()
                         │
                         ▼
                    Session::inject_dom_snapshot()
                         │
                         ▼
                    DomSnapshot::from_frame()  ← regenerates with mutations
```

## Threading Model

```
┌─────────────────────────────────────────────┐
│                 Tokio Runtime                 │
│                                              │
│  ┌──────────┐  ┌──────────┐  ┌───────────┐ │
│  │ CDP      │  │ Session  │  │ HttpClient│ │
│  │ Server   │  │ tasks    │  │ (reqwest) │ │
│  └──────────┘  └──────────┘  └───────────┘ │
│                                              │
│  ┌──────────────────────────────────────┐   │
│  │ TokioJobQueue                        │   │
│  │ (setTimeout / setInterval timers)    │   │
│  └──────────────────────────────────────┘   │
└─────────────────────────────────────────────┘

┌─────────────────────────────────────────────┐
│              JS Thread (std::thread)          │
│                                              │
│  ┌──────────────────────────────────────┐   │
│  │ boa_engine::Context (persistent)     │   │
│  │                                      │   │
│  │  • Web API closures                  │   │
│  │  • DomSnapshot (Arc<RwLock>)         │   │
│  │  • mpsc channels to main thread      │   │
│  └──────────────────────────────────────┘   │
└─────────────────────────────────────────────┘
```

**Why a dedicated JS thread?** `boa_engine::Context` is `!Send` — it cannot
cross thread boundaries. We run it on a dedicated `std::thread` and
communicate via `mpsc` channels.

## JS↔Rust Bridges

### DOM Bridge (DomSnapshot)

The DOM lives in the webapi crate (main thread). JS lives in boa (JS thread).
They communicate through a serialized `DomSnapshot`:

1. **Snapshot creation**: `Frame` → `DomSnapshot::from_frame()` → serialized HashMap of `DomNode`s
2. **Snapshot injection**: `Arc<RwLock<Option<DomSnapshot>>>` shared between threads
3. **Mutation collection**: `Arc<RwLock<Vec<DomMutation>>>` — JS writes, main thread drains
4. **Mutation application**: Main thread applies mutations to webapi DOM, then regenerates snapshot

### Network Bridge (fetch/XHR)

JS `fetch()` and `XMLHttpRequest` use `std::sync::mpsc` channels:

```
JS Thread                    Main Thread
────────                    ────────────
fetch(url)                  HttpClient::fetch(url)
    │                           │
    ├── Send FetchRequestMsg ──→│
    │                           ├── HTTP request
    │←── Recv FetchResponseMsg ─┤
    │                           │
    └── Resolve Promise         │
```

### Storage Bridge (localStorage)

```
JS Thread                    Session
────────                    ───────
localStorage.getItem(key)   Session::local_storage
    │                           │
    ├── Send StorageMsg::Get ──→│
    │←── Recv StorageValue ────┤
```

## Interior Mutability Strategy

| Data | Lock Type | Reason |
|------|-----------|--------|
| `Browser.sessions` | `parking_lot::RwLock` | Sync access to session list |
| `Session` in Browser | `tokio::sync::RwLock` | Async access per session |
| `CookieJar` | `parking_lot::RwLock` in `Arc` | Shared across HttpClient and Session |
| `DomSnapshot` | `parking_lot::RwLock` in `Arc` | Shared between JS thread and main |
| `DomMutation` vec | `parking_lot::RwLock` in `Arc` | JS writes, main thread drains |
| JS `localStorage` | `parking_lot::RwLock` in `Arc` | Cross-thread storage access |

## Error Handling

- **Library crates** (`oxibrowser-core`, `oxibrowser-webapi`, `oxibrowser-cdp`): `thiserror` for typed errors
- **Binary crate** (`oxibrowser`): `anyhow` for ergonomic error handling
- **JS runtime**: Errors are caught and returned as `JsResult` with exception details
- **CDP protocol**: Errors follow CDP spec format with `code` and `message`

## Security

| Feature | Implementation |
|---------|---------------|
| SSRF protection | `IpFilter` blocks private/reserved CIDR ranges (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, etc.) |
| robots.txt | `RobotStore` with RFC 9309 parser, opt-in via `--obey-robots` |
| CDP connection limit | Max 16 concurrent connections |
| CDP message size | Max 1 MB per message |
| JS runtime limits | Loop iterations, recursion depth, stack size, timeout |
| No sandbox escape | No native code execution, no file system access from JS |
