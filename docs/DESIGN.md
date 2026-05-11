# OxiBrowser Design Rationale

## Why Build OxiBrowser?

AI agents need a fast, lightweight, embeddable headless browser for web automation. Existing options have tradeoffs:

| Solution | Problem |
|----------|---------|
| **Chromium (Puppeteer)** | Heavy: ~200MB binary, 100MB+ RAM per instance, slow startup |
| **Firefox (Playwright)** | Similar resource footprint |
| **Lightpanda (Zig)** | Promising but Zig ecosystem is young; no Rust integration |
| **headless-chrome (Rust)** | Just wraps Chromium; same overhead |
| **fantoccini (Rust)** | WebDriver client; needs a browser binary |

OxiBrowser fills the gap: a **pure Rust** headless browser that's CDP-compatible, uses Servo ecosystem crates for web standards, and is designed for AI agent workloads.

---

## Why Servo Instead of Chromium?

### The Case for Servo

**Servo** is Mozilla's research browser engine, written in Rust. It provides:

1. **Rust-native:** No FFI overhead, no C++ build complexity, no V8 binary distribution
2. **Modular crates:** `html5ever`, `cssparser`, `string_cache`, `selectors` are all standalone crates
3. **Offscreen rendering:** Servo supports headless/offscreen rendering natively
4. **Memory safety:** Rust's ownership model eliminates entire classes of bugs
5. **Embeddable:** Servo can be embedded as a library, not just run as a standalone browser

### The Case Against Chromium

1. **Build complexity:** Building Chromium from source takes 30+ GB and hours
2. **Binary distribution:** Chromium binaries are 200MB+; not suitable for lightweight deployment
3. **FFI overhead:** Accessing Chromium internals from Rust requires C FFI
4. **Not designed for embedding:** Chromium's content API is C++, not Rust

### What We Actually Use From Servo

| Crate | What It Provides |
|-------|-----------------|
| `html5ever` | HTML5-compliant parser (TreeSink API) |
| `markup5ever` | Common types for markup parsers |
| `string_cache` | Interned strings for tag/attribute names |
| `cssparser` *(planned)* | CSS parsing and tokenization |
| `selectors` *(planned)* | Full CSS selector matching |
| `servo` *(planned, optional)* | Full rendering engine (layout, paint, compositing) |

---

## Why html5ever for HTML Parsing?

### Comparison with Alternatives

| Parser | Language | Spec Compliance | Speed | Ecosystem |
|--------|----------|----------------|-------|-----------|
| `html5ever` | Rust | HTML5 spec | Fast | Servo ecosystem |
| `lol_html` | Rust | HTML5 rewriter | Fast | Streaming |
| `scraper` | Rust | Wraps html5ever | Fast | Convenience layer |
| `select.rs` | Rust | Wraps html5ever | Fast | Simpler API |
| `quick-xml` | Rust | XML only | Very fast | Not HTML |
| Custom parser | Any | Incomplete | Varies | Fragile |

### Why html5ever Wins

1. **HTML5 spec compliance:** Handles malformed HTML the same way browsers do (quirks mode, error recovery)
2. **TreeSink API:** Perfect separation between parsing and tree building — we implement `TreeSink` to build our own `Document` type
3. **Servo heritage:** Battle-tested in a real browser engine
4. **Zero-copy tendril:** Uses `StrTendril` for efficient string handling during parsing
5. **No unsafe code:** Pure safe Rust

### Our TreeSink Implementation

We implement `html5ever::TreeSink` via `DomSink`:

```rust
struct DomSink {
    document: Document,
}

impl TreeSink for DomSink {
    type Handle = NodeId;
    // create_element → Node(Element{tag, attributes})
    // append → Tree::append_child
    // create_comment → Node(Comment)
    // etc.
}
```

This lets html5ever parse any HTML and build our `Document` type directly, with zero intermediate representations.

---

## Why tokio-tungstenite for CDP WebSocket?

### Comparison

| Library | Async | Performance | API Quality |
|---------|-------|-------------|-------------|
| `tokio-tungstenite` | tokio | High | Simple, well-maintained |
| `tungstenite` | Sync | High | Blocking, not suitable for async server |
| `fastwebsockets` | tokio | Very high | Lower-level, more complex API |
| `soketto` | Async (generic) | High | Good but less widely used |
| `ws-rs` | Sync | Medium | Older, less maintained |

### Why tokio-tungstenite

1. **tokio-native:** Integrates seamlessly with our async runtime
2. **Battle-tested:** Used in production by many Rust projects
3. **Simple API:** `WebSocketStream::accept()` → `Stream + Sink` → done
4. **TLS support:** Optional TLS via `rustls` or `native-tls` features
5. **Lightweight:** No unnecessary abstractions

---

## Comparison with Lightpanda

OxiBrowser's architecture is directly inspired by [Lightpanda](https://github.com/lightpanda-io/browser), a headless browser built in Zig. Here's what we ported, what we changed, and why.

### What We Ported (Architecture)

| Lightpanda Concept | OxiBrowser Equivalent | Notes |
|-------------------|----------------------|-------|
| `Browser.zig` | `browser.rs` | Owns sessions, HTTP client, cookie jar |
| `Session.zig` | `session.rs` | Browsing context with history, storage |
| `Page.zig` | `page.rs` | Loaded document with resources |
| `Frame.zig` | `frame.rs` | Parsed DOM with child frames |
| `src/cdp/` | `oxibrowser-cdp` | CDP protocol, domain dispatch |
| `src/cdp/domains/` | `domains/` | Per-domain handler modules |
| `src/dom/` | `oxibrowser-webapi` | DOM types and parsing |

### What We Changed

| Aspect | Lightpanda | OxiBrowser | Why |
|--------|-----------|------------|-----|
| Language | Zig | Rust | Rust ecosystem for web (Servo, reqwest, tokio), memory safety guarantees, better async |
| JS Engine | V8 (libv8 C bindings) | Stub → Servo (SpiderMonkey) | Avoids C FFI complexity; Servo's JS is Rust-accessible |
| HTML Parser | Custom + html5ever C bindings | html5ever (pure Rust) | Native Rust, no FFI, Servo ecosystem |
| HTTP Client | Custom | reqwest | Mature, well-tested, connection pooling, TLS |
| WebSocket | Custom | tokio-tungstenite | tokio integration, well-maintained |
| ID Generation | Sequential | AtomicU64/AtomicU32 | Thread-safe without locks |
| Cookie Jar | Per-session | Shared by default, Arc<RwLock<CookieJar>> | Cross-session cookie sharing for automation |
| Error Handling | Error unions | thiserror enum | Ergonomic, typed, convertible |
| Serialization | Custom JSON | serde_json | Standard, derive macros, zero-cost |
| Locking | Various | parking_lot + tokio::sync | parking_lot for sync, tokio for async |
| Rendering | No visual render | Servo offscreen (planned) | Future screenshot/PDF support |
| License | AGPL-3.0 | MIT | Permissive, broader adoption |

### What We Kept The Same

1. **CDP compatibility** — same protocol, same domains, same wire format
2. **Browser → Session → Page → Frame hierarchy** — proven, clean separation of concerns
3. **Domain dispatch pattern** — `Domain.method` routing to handler functions
4. **Stub-first approach** — get the architecture right, fill in implementations later
5. **Headless-first design** — no GUI, no display server dependency

---

## JS Runtime Abstraction Strategy

### Design Goals

1. **Zero-dependency default** — OxiBrowser should compile and run without any JS engine
2. **Pluggable real engine** — when `full-servo` feature is enabled, real JS execution via SpiderMonkey
3. **Consistent API** — same `JsRuntime` trait regardless of backend

### Architecture

```
┌────────────────────────────────────┐
│          JsRuntime                  │
│                                     │
│  evaluate(expr) → JsEvalResult     │
│  execute(script) → JsEvalResult    │
│  console_output() → &[String]      │
│  set_global(name, value)           │
├─────────────┬──────────────────────┤
│  Stub Mode  │   Servo Mode         │
│  (default)  │   (full-servo feat)  │
│             │                      │
│  Literals   │   SpiderMonkey       │
│  console.log│   evaluate_javascript│
│  globals    │   real DOM access    │
│  passthrough│   event loop         │
└─────────────┴──────────────────────┘
```

### Stub Mode Capabilities

The stub evaluator handles enough for basic CDP compatibility:

- String/number/boolean/null literals → direct conversion to `serde_json::Value`
- `console.log(...)` → captured to console buffer
- `document.title` / `document.URL` → returns empty string (placeholder)
- Global variable lookup → `HashMap<String, Value>`
- Unknown expressions → returned as string value (passthrough)

This is sufficient for Puppeteer/Playwright to connect and issue basic commands without crashing.

### Servo Mode Integration Path

When `full-servo` is enabled:

```rust
#[cfg(feature = "full-servo")]
pub struct JsRuntime {
    webview: servo::WebView,
    console: Vec<String>,
}

#[cfg(feature = "full-servo")]
impl JsRuntime {
    pub async fn evaluate(&mut self, expression: &str) -> Result<JsEvalResult> {
        match self.webview.evaluate_javascript(expression).await {
            Ok(value) => Ok(JsEvalResult::ok(serialize_servo_value(value))),
            Err(e) => Ok(JsEvalResult::error(e.to_string())),
        }
    }
}
```

The servo crate's `WebView::evaluate_javascript()` provides:
- Full ECMAScript support (via SpiderMonkey)
- DOM access (document, window, etc.)
- Event loop integration
- Promise handling

---

## Network Layer Design Decisions

### Why reqwest Over hyper Directly

`reqwest` is built on `hyper` but provides:
- **Connection pooling** built-in (`pool_max_idle_per_host`)
- **Cookie handling** (though we manage our own cookie jar for cross-session sharing)
- **Redirect policy** (configurable, defaults to 10 hops)
- **TLS** via `rustls` (no OpenSSL dependency)
- **Timeout management** at the client level
- **Builder API** for configuration

Using hyper directly would require reimplementing all of this.

### Why rustls Over native-tls

| Aspect | rustls | native-tls |
|--------|--------|------------|
| Dependencies | Pure Rust | OpenSSL / Security.framework |
| Cross-compile | Easy | Painful |
| Performance | Comparable | Comparable |
| Safety | Memory-safe | C FFI |
| Audit | Auditable | Black box |

For a headless browser designed for automation and AI agents, minimizing system dependencies is critical.

### Cookie Jar Design

The cookie jar is intentionally simple:
- `HashMap<String, Vec<String>>` — domain → raw Set-Cookie values
- No expiration tracking (yet)
- No HttpOnly/Secure enforcement (yet)
- No SameSite handling (yet)
- Shared across sessions by default (common automation pattern)

This covers 90% of automation use cases. Full RFC 6265 compliance can be added incrementally.

---

## Roadmap

### Phase 1: Core Foundation ✅ (Current)

- [x] Browser → Session → Page → Frame hierarchy
- [x] HTML parsing via html5ever
- [x] DOM querying (CSS selectors: tag, .class, #id)
- [x] HTTP client with cookie jar
- [x] JS runtime stub
- [x] CDP protocol types
- [x] CDP domain dispatch skeleton
- [x] Error types and propagation

### Phase 2: CDP Server (In Progress)

- [x] CdpServer: hyper HTTP for `/json/*` endpoints
- [ ] CdpServer: tokio-tungstenite WebSocket handler (wire WS upgrade to CdpSession)
- [x] CdpSession: per-connection state management
- [x] Domain implementations: Browser, Page, Target
- [x] Domain implementations: Runtime, DOM, Network
- [ ] Wire domain handlers to core Browser (currently returns static JSON stubs)
- [ ] Event emission (loadEventFired, frameNavigated, etc.)
- [x] Binary entry point with CLI (`oxibrowser serve`, `oxibrowser fetch`)

### Phase 3: Enhanced CDP

- [ ] Full CSS selector support (via `selectors` crate from Servo)
- [ ] Network interception (Fetch domain)
- [ ] Screenshot support (PNG via image crate)
- [ ] PDF generation
- [ ] Multi-session cookie isolation
- [ ] Cookie expiration / SameSite / Secure enforcement
- [ ] iframe loading and frame tree management

### Phase 4: Servo Integration

- [ ] Enable `full-servo` feature flag
- [ ] Wire `servo::WebView` into `JsRuntime`
- [ ] Real JavaScript execution
- [ ] DOM mutation from JS (reflect changes in our Document)
- [ ] CSS parsing via `cssparser`
- [ ] Layout computation
- [ ] Offscreen rendering pipeline
- [ ] Accurate screenshots with rendered output

### Phase 5: Production Hardening

- [ ] Full RFC 6265 cookie compliance
- [ ] Navigation timeout enforcement
- [ ] Resource loading pipeline (scripts, stylesheets, images)
- [ ] HTTP/2 support
- [ ] WebSocket client support (for testing WS apps)
- [ ] Performance benchmarking vs. Chromium headless
- [ ] Fuzz testing for HTML parser edge cases
- [ ] Puppeteer integration test suite
- [ ] Playwright integration test suite
