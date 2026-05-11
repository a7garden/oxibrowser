# OxiBrowser Architecture

## System Overview

```
                              ┌─────────────────────────┐
                              │   Puppeteer / Playwright │
                              │   (CDP WebSocket client) │
                              └────────────┬────────────┘
                                           │ WebSocket
                                           ▼
┌──────────────────────────────────────────────────────────────────┐
│                        oxibrowser-cdp                            │
│  ┌─────────────┐    ┌──────────────┐    ┌──────────────────┐   │
│  │ HTTP Server │    │ CdpSession   │    │ Domain Dispatch  │   │
│  │ /json       │    │ (per-WS conn)│    │ Browser          │   │
│  │ /json/version│   │              │    │ DOM              │   │
│  └──────┬──────┘    └──────┬───────┘    │ Network          │   │
│         │                  │            │ Page             │   │
└─────────┼──────────────────┼────────────│ Runtime          │───┘
          │                  │            │ Target           │
          │                  ▼            └────────┬─────────┘
          │        ┌─────────────────┐             │
          │        │  oxibrowser-core│             │
          │        │                 │             │
          │        │  ┌───────────┐  │             │
          │        │  │  Browser  │  │             │
          │        │  │  (top)    │  │             │
          │        │  └─────┬─────┘  │             │
          │        │        │ owns   │             │
          │        │  ┌─────▼─────┐  │             │
          │        │  │  Session  │  │             │
          │        │  │  (1..N)   │  │             │
          │        │  └─────┬─────┘  │             │
          │        │        │ owns   │             │
          │        │  ┌─────▼─────┐  │             │
          │        │  │   Page    │  │             │
          │        │  │  (0..1)   │  │             │
          │        │  └─────┬─────┘  │             │
          │        │        │ owns   │             │
          │        │  ┌─────▼─────┐  │             │
          │        │  │   Frame   │──┼──┐          │
          │        │  │ (root)    │  │  │          │
          │        │  └─────┬─────┘  │  │          │
          │        │    ┌───┴───┐    │  │          │
          │        │    │ Frame │    │  │          │
          │        │    │(child)│    │  │          │
          │        │    └───────┘    │  │          │
          │        │                 │  │          │
          │        │  ┌───────────┐  │  │          │
          │        │  │ JsRuntime │  │  │          │
          │        │  │ (per-sess)│  │  │          │
          │        │  └───────────┘  │  │          │
          │        │                 │  │          │
          │        │  ┌───────────┐  │  │          │
          │        │  │HttpClient │  │  │          │
          │        │  │(reqwest)  │  │  │          │
          │        │  └─────┬─────┘  │  │          │
          │        │        │        │  │          │
          │        │  ┌─────▼─────┐  │  │          │
          │        │  │ CookieJar │  │  │          │
          │        │  │(per-domain│  │  │          │
          │        │  └───────────┘  │  │          │
          │        └─────────────────┘  │          │
          │                             │          │
          │        ┌────────────────────┘          │
          │        │  oxibrowser-webapi            │
          │        │                               │
          │        │  ┌──────────┐  ┌───────────┐ │
          │        │  │ Document │  │   Tree     │ │
          │        │  │(html5ever│  │(adjacency  │ │
          │        │  │  parse)  │  │  list)     │ │
          │        │  └────┬─────┘  └─────┬─────┘ │
          │        │       │              │        │
          │        │       ▼              ▼        │
          │        │  ┌──────────────────────┐    │
          │        │  │       Node           │    │
          │        │  │ Document|Element|    │    │
          │        │  │ Text|Comment|Doctype │    │
          │        │  └──────────────────────┘    │
          │        └───────────────────────────────┘
          │
          ▼
     ┌─────────┐
     │  Target │ (remote web server)
     └─────────┘
```

## Component Responsibilities

### oxibrowser-webapi

The foundational layer. Provides DOM types and parsing with zero async runtime dependency.

| Type | Responsibility |
|------|---------------|
| `Document` | HTML parsing via html5ever `TreeSink`, CSS selector queries (`query_selector`, `query_selector_all`), text extraction, Markdown conversion |
| `Tree` | Adjacency-list parent/child relationships, DFS/BFS traversal, root management |
| `Node` | Individual DOM node with `NodeType` (Document, Element, Text, Comment, Doctype), attribute access |
| `NodeId(usize)` | Unique identifier within a `Document` |
| `DomSink` | html5ever `TreeSink` implementation that builds our `Document` from HTML |

**Key design:** `Document` owns all nodes in a `HashMap<NodeId, Node>`. `Tree` stores structural relationships separately. This separates data from structure, making queries efficient and mutations straightforward.

### oxibrowser-core

The browser engine. Manages the full lifecycle of browsing.

| Module | Responsibility |
|--------|---------------|
| `Browser` | Top-level instance: owns sessions list, HTTP client, cookie jar, config. Enforces max sessions, prevents operations after close. |
| `Session` | Browsing context: navigation (fetch → parse → build Page), history (back/forward/reload), local storage, JS runtime. |
| `Page` | Loaded document: URL, status code, content-type, root Frame, sub-resources, title. |
| `Frame` | Document frame: parsed `Document`, raw HTML, child frames, DOM version counter. |
| `JsRuntime` | JavaScript evaluation: stub mode (literals, console.log, globals) or servo mode (planned). |
| `HttpClient` | HTTP client: wraps reqwest, injects cookies, follows redirects, stores response cookies. |
| `CookieJar` | Cookie storage: domain-scoped HashMap, store/lookup/clear. |
| `Resource` | Sub-resource tracking: URL, type, status, MIME, body bytes, load timestamp. |
| `BrowserConfig` | Configuration: user-agent, timeout, viewport, pool size, TLS, robots.txt. |
| `CoreError` | Error enum: NavigationFailed, JsError, NetworkError, PageError, SessionError, Timeout, BrowserClosed, InvalidUrl, DomError. |

### oxibrowser-cdp

The automation interface. Implements Chrome DevTools Protocol.

| Module | Responsibility |
|--------|---------------|
| `CdpServer` | ✅ Implemented (HTTP only) | HTTP server for `/json/*` endpoints; WebSocket upgrade handler is a stub |
| `CdpSession` | ✅ Implemented | Per-WebSocket-connection state, method dispatch, event subscription |
| `protocol` | ✅ Complete | Wire types: `CdpRequest`, `CdpResponse`, `CdpEvent`, `JsonVersion`, `JsonTarget`, error codes |
| `domains::dispatch()` | ✅ Complete | Routes `Domain.method` strings to handler functions |
| `domains::browser` | ✅ Stub | Browser domain: getVersion, getWindowForTarget, close |
| `domains::dom` | ✅ Stub | DOM domain: getDocument, describeNode, querySelector, querySelectorAll, getOuterHTML, resolveNode |
| `domains::network` | ✅ Stub | Network domain: enable, disable, loadResource, getResponseBody |
| `domains::page` | ✅ Stub | Page domain: navigate, reload, getFrameTree, getFrameMetrics, captureScreenshot, printToPDF |
| `domains::runtime` | ✅ Stub | Runtime domain: enable, disable, evaluate, callFunctionOn, getProperties, compileScript, runScript |
| `domains::target` | ✅ Stub | Target domain: setAutoAttach, attachToTarget, detachFromTarget, createTarget, closeTarget, getTargetInfo, getTargets, setDiscoverTargets |

## Data Flow: Typical Page Load

```
  Puppeteer: page.goto("https://example.com")
       │
       ▼
  CDP WebSocket: {"id":1, "method":"Page.navigate", "params":{"url":"https://example.com"}}
       │
       ▼
  CdpServer receives on WebSocket
       │
       ▼
  CdpSession deserializes → CdpRequest { id: 1, method: "Page.navigate", params: {...} }
       │
       ▼
  dispatch("Page.navigate", params) → domains::page::handle("navigate", params)
       │
       ▼
  page::navigate() calls Browser → Session → navigate(url)
       │
       ▼
  Session::navigate(url)
    │
    ├─1─ Url::parse(url)
    │
    ├─2─ HttpClient::fetch(&parsed_url)
    │      │
    │      ├─ CookieJar::cookies_for_url() → inject Cookie header
    │      │
    │      ├─ reqwest GET request
    │      │
    │      └─ CookieJar::store() → store Set-Cookie response
    │
    ├─3─ response.text() → HTML string
    │
    ├─4─ Page::from_html(url, html, status, content_type)
    │      │
    │      ├─ Frame::from_html(url, html)
    │      │    │
    │      │    └─ Document::parse(html)
    │      │         │
    │      │         ├─ html5ever parse_document(DomSink)
    │      │         │    └─ DomSink (TreeSink) builds Document:
    │      │         │         create_element → Node(Element{tag, attrs})
    │      │         │         append → Tree::append_child
    │      │         │         create_comment → Node(Comment)
    │      │         │         etc.
    │      │         │
    │      │         └─ Returns Document with populated nodes + tree
    │      │
    │      └─ Frame::extract_title() → Document::query_text("title")
    │
    ├─5─ Update history: push URL, set history_index
    │
    └─6─ Set active_page = Some(page)
       │
       ▼
  CdpSession serializes → CdpResponse { id: 1, result: {"frameId": "...", "loaderId": "..."} }
       │
       ▼
  CdpServer sends on WebSocket
       │
       ▼
  Puppeteer receives response, resolves page.goto() promise
```

## CDP Protocol Message Lifecycle

### WebSocket Connection

```
1. Client connects to ws://host:port
2. Client sends: {"id": 1, "method": "Target.attachToTarget", ...}
3. Server responds: {"id": 1, "result": {"sessionId": "abc123"}}
4. Subsequent messages include "sessionId": "abc123"
```

### Message Format

**Request (client → server):**
```json
{
  "id": 1,
  "method": "Page.navigate",
  "params": {"url": "https://example.com"},
  "sessionId": "abc123"
}
```

**Response (server → client):**
```json
{
  "id": 1,
  "result": {"frameId": "frame-1", "loaderId": "loader-1"},
  "sessionId": "abc123"
}
```

**Event (server → client, unsolicited):**
```json
{
  "method": "Page.loadEventFired",
  "params": {"timestamp": 1234567890},
  "sessionId": "abc123"
}
```

**Error response:**
```json
{
  "id": 1,
  "error": {"code": -32601, "message": "unknown method: Foo.bar"},
  "sessionId": "abc123"
}
```

### HTTP Endpoints

| Endpoint | Returns | Purpose |
|----------|---------|---------|
| `GET /json/version` | `JsonVersion` | Browser metadata, WebSocket URL |
| `GET /json` | `Vec<JsonTarget>` | List of debuggable targets (pages) |

### Error Codes

| Code | Constant | Meaning |
|------|----------|---------|
| -32700 | `PARSE_ERROR` | Invalid JSON |
| -32600 | `INVALID_REQUEST` | Malformed request |
| -32601 | `METHOD_NOT_FOUND` | Unknown domain or method |
| -32602 | `INVALID_PARAMS` | Missing or invalid parameters |
| -32603 | `INTERNAL_ERROR` | Server-side error |
| -32000 | `SERVER_ERROR` | Application-specific error |

### Dispatch Flow

```rust
// In domains/mod.rs
pub fn dispatch(method: &str, params: Option<Value>) -> DomainResult {
    let parts: Vec<&str> = method.splitn(2, '.').collect();
    let (domain, method_name) = (parts[0], parts[1]);

    match domain {
        "Browser" => browser::handle(method_name, params),
        "DOM"     => dom::handle(method_name, params),
        "Network" => network::handle(method_name, params),
        "Page"    => page::handle(method_name, params),
        "Runtime" => runtime::handle(method_name, params),
        "Target"  => target::handle(method_name, params),
        _ => Err(CdpError { code: -32601, message: format!("unknown domain: {domain}") }),
    }
}
```

## JS Runtime Abstraction

### Stub Mode (Default)

The stub evaluator handles simple expressions without a real JS engine:

| Input | Output |
|-------|--------|
| `"hello"` (string literal) | `Value::String("hello")` |
| `42` (integer) | `Value::Number(42)` |
| `3.14` (float) | `Value::Number(3.14)` |
| `true` / `false` | `Value::Bool` |
| `null` | `Value::Null` |
| `console.log("msg")` | Void result, appends to console buffer |
| `document.title` | `Value::String("")` (placeholder) |
| `document.URL` | `Value::String("")` (placeholder) |
| Known global variable | `Value` from globals HashMap |
| Unknown expression | `Value::String(expression)` (passthrough) |

### Servo Mode (Planned, `full-servo` feature)

When the `full-servo` feature is enabled:

```rust
// Future implementation
pub async fn evaluate(&mut self, expression: &str) -> Result<JsEvalResult> {
    let result = self.webview.evaluate_javascript(expression).await?;
    // Convert servo result to JsEvalResult
}
```

This would use Servo's SpiderMonkey (or V8) engine for real JavaScript execution.

### JsEvalResult Structure

```rust
pub struct JsEvalResult {
    pub value: Option<Value>,          // JSON return value
    pub exception: Option<String>,     // Error message
    pub console_output: Vec<String>,   // Captured console.log output
}
```

## Network Layer Design

### HttpClient

```
┌──────────────────────────────────────┐
│           HttpClient                 │
│                                      │
│  ┌────────────────────────────────┐  │
│  │    reqwest::Client             │  │
│  │  - User-Agent header           │  │
│  │  - Connection pool (N/host)    │  │
│  │  - Redirect policy (max 10)    │  │
│  │  - Timeout (from config)       │  │
│  │  - Optional: accept invalid TLS│  │
│  └────────────────────────────────┘  │
│                                      │
│  fetch(url):                         │
│    1. CookieJar → Cookie header      │
│    2. reqwest GET                    │
│    3. Set-Cookie → CookieJar.store() │
│    4. Return Response                │
└──────────────────────────────────────┘
```

### Cookie Jar

```
CookieJar
├── HashMap<String, Vec<String>>
│   "example.com" → ["session=abc; Path=/", "lang=en; Max-Age=3600"]
│   "api.example.com" → ["token=xyz"]
│
├── store(url, set_cookie_header)
│   Extract domain from URL host
│   Push raw Set-Cookie value
│
└── cookies_for_url(url)
    Extract domain from URL host
    Parse each cookie: split on ';', take first part
    Join with "; "
```

### Resource Tracking

```rust
pub enum ResourceType {
    Document,    // HTML pages
    Script,      // <script src>
    Stylesheet,  // <link rel="stylesheet">
    Image,       // <img>, background-image
    Font,        // @font-face
    Xhr,         // XMLHttpRequest
    Fetch,       // fetch() API
    WebSocket,   // ws:// connections
    Other(String), // Fallback
}

pub struct Resource {
    pub url: String,
    pub resource_type: ResourceType,
    pub status: u16,
    pub mime_type: String,
    pub body: Bytes,
    pub loaded_at: Instant,
}
```

## Session / Page / Frame Lifecycle State Machines

### Browser Lifecycle

```
                new()
                  │
                  ▼
          ┌───────────────┐
          │    Open        │◄─────┐
          │ sessions: []   │      │
          │ closed: false  │      │
          └───────┬───────┘      │
                  │              │
        new_session()            │
                  │              │
                  ▼              │
          ┌───────────────┐     │
          │ Open           │     │
          │ sessions: [S1] │     │
          └───────┬───────┘     │
                  │              │
           close()               │
                  │              │
                  ▼              │
          ┌───────────────┐     │
          │    Closed      │     │
          │ closed: true   │─────┘ (Drop warns if not closed)
          └───────────────┘
```

### Session Lifecycle

```
             new()
               │
               ▼
       ┌──────────────┐
       │  Empty        │
       │  page: None   │
       │  history: []  │
       └──────┬───────┘
              │
        navigate(url)
              │
              ├─ 1. Parse URL
              ├─ 2. Fetch URL
              ├─ 3. Parse HTML → Page → Frame → Document
              ├─ 4. Push to history
              └─ 5. Set active_page
              │
              ▼
       ┌──────────────┐
       │  Loaded       │
       │  page: Some   │◄────────────────┐
       │  history: [U1]│                  │
       └──────┬───────┘                  │
              │                          │
     navigate(url2)                      │
              │                          │
              ▼                          │
       ┌──────────────┐                 │
       │  Loaded       │                 │
       │  page: Some   │                 │
       │  history:[U1,U2]               │
       └──────┬───────┘                 │
              │                          │
     go_back() │ go_forward() │ reload() │
              │     │            │       │
              └─────┴────────────┴───────┘
              (re-fetch, update active_page)

              │
        close()
              │
              ▼
       ┌──────────────┐
       │  Closed       │
       │  page: None   │
       │  history: []  │
       └──────────────┘
```

### Page Lifecycle

```
  Page::from_html(url, html, status, content_type)
               │
               ├─ Frame::from_html(url, html)
               │    └─ Document::parse(html)
               │         └─ html5ever builds DOM tree
               │
               ├─ Extract title
               │
               ▼
       ┌──────────────┐
       │  Page         │
       │  root_frame   │
       │  resources:[] │
       └──────────────┘
```

### Frame Lifecycle

```
  Frame::from_html(url, html)
               │
               ▼
       ┌──────────────┐
       │  Frame        │
       │  document     │ (parsed DOM)
       │  html         │ (raw source)
       │  children: [] │
       │  dom_version:0│
       └──────┬───────┘
              │
   add_child(frame)  │  document_mut()
              │      │      │
              ▼      ▼      ▼
       Increment dom_version on mutation
```

## Error Propagation Strategy

```
oxibrowser-webapi (no errors defined, uses panics for internal invariants)
        │
        │ consumed by
        ▼
oxibrowser-core (CoreError via thiserror)
        │
        │  ┌─ CoreError::NavigationFailed  ← Session::navigate failures
        │  ├─ CoreError::JsError            ← JsRuntime evaluation failures
        │  ├─ CoreError::NetworkError       ← HttpClient / reqwest failures
        │  ├─ CoreError::PageError          ← Page construction failures
        │  ├─ CoreError::SessionError       ← Session limit, state errors
        │  ├─ CoreError::Timeout            ← Operation timeouts
        │  ├─ CoreError::BrowserClosed      ← Operations on closed browser
        │  ├─ CoreError::InvalidUrl         ← URL parse failures
        │  └─ CoreError::DomError           ← DOM operation failures
        │
        │ consumed by
        ▼
oxibrowser-cdp (CdpError in DomainResult)
        │
        │  CdpError { code: -32xxx, message: "..." }
        │  Wrapped in CdpResponse::error
        │  Sent as JSON over WebSocket
```

External error conversions:
- `url::ParseError` → `CoreError::InvalidUrl`
- `reqwest::Error` → `CoreError::NetworkError`

## Thread / Async Model

### Tokio Runtime

OxiBrowser uses a single tokio runtime with multiple async tasks:

```
┌─────────────────────────────────────────────────┐
│                 tokio Runtime                    │
│                                                  │
│  Task 1: CDP HTTP Server                         │
│    - hyper HTTP listener                         │
│    - /json, /json/version handlers               │
│                                                  │
│  Task 2: CDP WebSocket connections               │
│    - tokio-tungstenite per connection             │
│    - CdpSession per connection                   │
│    - Message read loop → dispatch → write        │
│                                                  │
│  Task 3+: Session operations                     │
│    - navigate() → HttpClient::fetch() (reqwest)  │
│    - evaluate_js()                               │
│    - Concurrent page loads across sessions       │
│                                                  │
└─────────────────────────────────────────────────┘
```

### Synchronization Strategy

| Data | Guard | Rationale |
|------|-------|-----------|
| `Browser.sessions` | `parking_lot::RwLock` | Fast reads (iterate sessions), infrequent writes (add/remove) |
| Individual `Session` | `tokio::sync::RwLock` wrapped in `Arc` | Session operations are async (navigate, evaluate) |
| `CookieJar` | `parking_lot::RwLock` inside `Arc` | Shared between HttpClient and Session, sync access sufficient |
| `Browser.closed` | `AtomicBool` | Single flag, lock-free |
| ID counters | `AtomicU64` / `AtomicU32` | Lock-free unique ID generation |
| DOM version | `u64` on Frame | Single-threaded access (via Session tokio::RwLock) |

### Why parking_lot for CookieJar and Session List

- `parking_lot::RwLock` is not async-aware but has lower overhead than `tokio::sync::RwLock`
- Cookie jar operations are fast (HashMap lookup) — never hold across await points
- Session list operations are fast (push, iterate) — never hold across await points
- Individual Session access uses `tokio::sync::RwLock` because Session methods call `.await`
