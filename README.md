# OxiBrowser

A headless browser built in pure Rust, designed for AI agents and automation.

Inspired by [Lightpanda](https://github.com/lightpanda-io/browser), but fully Rust-native — no Node.js, no V8 C bindings, no non-Rust dependencies.

## Architecture

```
┌─────────────────────────────────────────────────┐
│             Puppeteer / Playwright / Rust CDP    │
│             (any CDP-compatible client)          │
└────────────────────┬────────────────────────────┘
                     │ CDP WebSocket
                     ▼
┌─────────────────────────────────────────────────┐
│              CDP Server (oxibrowser-cdp)         │
│  HTTP: /json/version, /json                      │
│  WebSocket: command dispatch + event broadcasting │
│  Domains: Browser, DOM, Fetch, Network,          │
│           Page, Runtime, Target                  │
├─────────────────────────────────────────────────┤
│           Core Engine (oxibrowser-core)          │
│  Browser → Session → Page → Frame                │
│  HTTP Client (reqwest) · Cookie Jar              │
│  JS Runtime (stub · servo planned)               │
├─────────────────────────────────────────────────┤
│            WebAPI (oxibrowser-webapi)             │
│  DOM (html5ever) · CSS selectors · Markdown      │
│  Resource URL extraction (script, css, img, iframe)│
└─────────────────────────────────────────────────┘
```

## Features

- **CDP-compatible** — Puppeteer and Playwright can connect via WebSocket
- **Event broadcasting** — Page, Network, Runtime, Fetch domain events
- **Real DOM** — html5ever (Servo ecosystem) parses HTML into queryable DOM
- **CSS selectors** — tag, `.class`, `#id`, `tag.class`, `tag#id`
- **Cookie jar** — domain-scoped storage with automatic injection
- **Network interception** — Fetch domain with `requestPaused` events
- **Markdown conversion** — DOM to Markdown for AI agent consumption
- **Pure Rust** — zero non-Rust dependencies (no Node.js, no V8 C bindings)

## Crates

| Crate | Purpose |
|-------|---------|
| `oxibrowser` | Binary + CLI (`fetch`, `serve`, `version`) |
| `oxibrowser-core` | Browser lifecycle: Browser → Session → Page → Frame |
| `oxibrowser-cdp` | CDP server (HTTP + WebSocket + domain handlers) |
| `oxibrowser-webapi` | DOM parsing, CSS selectors, Markdown, resource extraction |

## Status

| Component | Status |
|-----------|--------|
| Browser → Session → Page → Frame | ✅ Complete |
| HTML parsing (html5ever) | ✅ Complete |
| DOM querying (CSS selectors) | ✅ Complete |
| Markdown conversion | ✅ Complete |
| HTTP client + cookie jar | ✅ Complete |
| CDP server (HTTP + WebSocket) | ✅ Complete (RFC 6455) |
| CDP domains (7 domains) | ✅ Complete |
| CDP event broadcasting | ✅ Complete |
| Network interception (Fetch) | ✅ Notification-only |
| Sub-resource extraction | ✅ Complete |
| E2E tests (pure Rust) | ✅ 15 tests |
| JS runtime (stub mode) | ✅ Literals, console.log, globals |
| JS runtime (servo mode) | 🔲 Planned |
| Servo offscreen rendering | 🔲 Planned |

### CDP Domains

| Domain | Key Methods | Events |
|--------|-------------|--------|
| Browser | getVersion, close | — |
| DOM | getDocument, querySelector, querySelectorAll, getOuterHTML | — |
| Fetch | enable, disable, continueRequest, failRequest, fulfillRequest | requestPaused |
| Network | enable, disable | requestWillBeSent, responseReceived, loadingFinished |
| Page | navigate, reload, getFrameTree, captureScreenshot | frameNavigated, loadEventFired, domContentLoadedEventFired |
| Runtime | evaluate, callFunctionOn, getProperties | executionContextCreated, consoleAPICalled |
| Target | createTarget, getTargets, attachToTarget | — |

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

Then connect with any CDP-compatible client to `ws://127.0.0.1:9222/ws`.

### Connect with Puppeteer (Node.js)

```javascript
import puppeteer from 'puppeteer-core';

const browser = await puppeteer.connect({
  browserWSEndpoint: 'ws://127.0.0.1:9222/ws',
});
const page = await browser.newPage();
await page.goto('https://example.com');
const title = await page.title();
console.log(title);
await browser.close();
```

### As a Rust library

```rust
use oxibrowser_core::{Browser, BrowserConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let browser = Browser::new(BrowserConfig::default()).await?;
    let session = browser.new_page("https://example.com").await?;

    let guard = session.read().await;
    if let Some(page) = guard.page() {
        println!("Title: {:?}", page.title());
        println!("Markdown: {}", page.to_markdown());
    }

    browser.close().await?;
    Ok(())
}
```

## Build & Test

```bash
cargo build                              # Build everything
cargo test --workspace                   # 53 tests (unit + E2E)
cargo clippy --workspace --all-targets   # Zero warnings
cargo run -- fetch https://example.com   # Fetch a page
cargo run -- serve --port 9222          # Start CDP server
```

## Comparison with Lightpanda

| Feature | Lightpanda | OxiBrowser |
|---------|-----------|------------|
| Language | Zig | Rust |
| JS Engine | V8 (C++ bindings) | Stub (Servo planned) |
| HTML Parser | html5ever | html5ever (same) |
| HTTP Client | libcurl | reqwest (Rust-native) |
| CDP | Full (20+ domains) | 7 core domains |
| Event System | Custom Notification | mpsc + AtomicBool |
| Rendering | No visual render | Servo offscreen (planned) |
| License | AGPL-3.0 | MIT |

## License

MIT
