# OxiBrowser

A headless browser built in pure Rust, powered by the Servo engine.

Designed for AI agents and automation — inspired by [Lightpanda](https://github.com/lightpanda-io/browser), but fully Rust-native.

## Architecture

```
┌─────────────────────────────────────────────────┐
│                    OxiBrowser                    │
├─────────────────────────────────────────────────┤
│  CDP Server (WebSocket, Puppeteer/Playwright)    │
├─────────────────────────────────────────────────┤
│  Browser → Session → Page → Frame               │
├──────────┬──────────┬───────────────────────────┤
│  WebAPI  │  Network │  JS Runtime (Servo V8/SM) │
│  DOM     │  HTTP    │  evaluate_javascript()     │
│  CSS     │  WS      │  event loop                │
│  Storage │  Cache   │                            │
├──────────┴──────────┴───────────────────────────┤
│              Servo Engine (Rendering)            │
│         html5ever · cssparser · offscreen        │
└─────────────────────────────────────────────────┘
```

## Crates

| Crate | Purpose |
|-------|---------|
| `oxibrowser` | Binary + CLI entry point |
| `oxibrowser-core` | Browser, Session, Page, Frame lifecycle |
| `oxibrowser-cdp` | Chrome DevTools Protocol server |
| `oxibrowser-webapi` | DOM, WebAPI implementations (backed by Servo/html5ever) |

## Status

| Component | Status |
|-----------|--------|
| Browser → Session → Page → Frame | ✅ Implemented |
| HTML parsing (html5ever) | ✅ Implemented |
| DOM querying (CSS selectors) | ✅ Implemented |
| Markdown conversion | ✅ Implemented |
| HTTP client + cookie jar | ✅ Implemented |
| CDP protocol types | ✅ Implemented |
| CDP domain dispatch (6 domains) | ✅ Stub (returns static JSON) |
| CDP server (HTTP endpoints) | ✅ Implemented |
| CDP server (WebSocket) | 🔲 Pending |
| CLI (`fetch`, `serve`, `version`) | ✅ Implemented |
| JS runtime (stub mode) | ✅ Implemented |
| JS runtime (servo mode) | 🔲 Planned |
| Servo offscreen rendering | 🔲 Planned |

## Usage

### Fetch a page (dump HTML)

```bash
oxibrowser fetch https://example.com
```

### Fetch and convert to Markdown

```bash
oxibrowser fetch https://example.com --format markdown
```

### Start CDP server

```bash
oxibrowser serve --host 127.0.0.1 --port 9222
```

Then connect with Puppeteer/Playwright to `ws://127.0.0.1:9222`.

### As a library

```rust
use oxibrowser_core::{Browser, BrowserConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let browser = Browser::new(BrowserConfig::default()).await?;
    let session = browser.new_page("https://example.com").await?;
    
    let session_guard = session.read().await;
    if let Some(page) = session_guard.page() {
        println!("Title: {:?}", page.title());
        println!("HTML: {}", page.content());
        println!("Markdown: {}", page.to_markdown());
    }
    
    browser.close().await?;
    Ok(())
}
```

## Comparison with Lightpanda

| Feature | Lightpanda | OxiBrowser |
|---------|-----------|------------|
| Language | Zig | Rust |
| JS Engine | V8 (libv8 C bindings) | Servo (SpiderMonkey via servo crate) |
| HTML Parser | Custom + html5ever | html5ever (Servo ecosystem) |
| Rendering | No visual render | Servo offscreen rendering (planned) |
| CDP | ✅ Full | ✅ Stub (6 domains) |
| Stars | 30k+ | New |
| License | AGPL-3.0 | MIT |

## Documentation

| Doc | Description |
|-----|-------------|
| [AGENTS.md](AGENTS.md) | Convention guide for AI agents |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Deep architecture document |
| [docs/CDP.md](docs/CDP.md) | CDP protocol implementation guide |
| [docs/DESIGN.md](docs/DESIGN.md) | Design rationale and roadmap |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Contribution guide |

## Build & Run

```bash
cargo build                    # Build everything
cargo test --workspace         # Run all tests
cargo run -- fetch https://example.com     # Fetch a page
cargo run -- serve --port 9222             # Start CDP server
```

## License

MIT
