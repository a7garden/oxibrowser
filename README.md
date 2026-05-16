<div align="center">

# 🌐 OxiBrowser

**The headless browser built in pure Rust for AI agents.**

Not a Chromium fork. Not a C++ wrapper. A browser engine written from scratch in Rust,
designed from day one for automation, web scraping, and AI-driven workflows.

[![crates.io](https://img.shields.io/crates/v/oxibrowser?style=flat-square&logo=rust&color=orange)](https://crates.io/crates/oxibrowser)
[![docs.rs](https://img.shields.io/docsrs/oxibrowser?style=flat-square&color=blue)](https://docs.rs/oxibrowser)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](https://github.com/a7garden/oxibrowser/blob/main/LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/a7garden/oxibrowser?style=flat-square&logo=github&color=yellow)](https://github.com/a7garden/oxibrowser/stargazers)
[![CI](https://img.shields.io/github/actions/workflow/status/a7garden/oxibrowser/ci.yml?branch=main&style=flat-square&logo=github)](https://github.com/a7garden/oxibrowser/actions)
[![Rust](https://img.shields.io/badge/rust-1.82%2B-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)

[Report Bug](https://github.com/a7garden/oxibrowser/issues) · [Request Feature](https://github.com/a7garden/oxibrowser/issues) · [Read the Docs](https://github.com/a7garden/oxibrowser/blob/main/docs/ARCHITECTURE.md) · [Discord](https://discord.gg/oxibrowser)

</div>

---

<div align="center">

<table>
<tr>
<td align="center"><strong>21 MB</strong><br><sub>Single static binary</sub></td>
<td align="center"><strong>~50 ms</strong><br><sub>Cold start time</sub></td>
<td align="center"><strong>~8 MB</strong><br><sub>Base memory</sub></td>
<td align="center"><strong>279 tests</strong><br><sub>Full coverage</sub></td>
<td align="center"><strong>Zero C deps</strong><br><sub>Pure Rust</sub></td>
</tr>
</table>

<table>
<tr>
<th>OxiBrowser</th>
<th>Headless Chrome</th>
<th>Lightpanda</th>
</tr>
<tr>
<td align="center">21 MB binary</td>
<td align="center">~400 MB install</td>
<td align="center">~80 MB binary</td>
</tr>
<tr>
<td align="center">~8 MB RAM base</td>
<td align="center">~200 MB RAM base</td>
<td align="center">~30 MB RAM base</td>
</tr>
<tr>
<td align="center">~50 ms startup</td>
<td align="center">~800 ms startup</td>
<td align="center">~10 ms startup</td>
</tr>
<tr>
<td align="center">Pure Rust (boa)</td>
<td align="center">C++ (V8)</td>
<td align="center">Zig (V8)</td>
</tr>
<tr>
<td align="center">MIT License</td>
<td align="center">BSD / ToS</td>
<td align="center">AGPL-3.0</td>
</tr>
</table>

</div>

---

## ✨ Why OxiBrowser?

**You're building AI agents that need to browse the web.** You don't need a full browser with GPU rendering, audio output, and extension support. You need something fast, small, and programmable.

OxiBrowser is built for exactly that use case:

- 🤖 **AI-Agent First** — Native `OXI` CDP domain with `getMarkdown()`, `getPageInfo()`, and text-first rendering
- ⚡ **Blazing Fast** — Cold starts in ~50ms, no Chromium overhead, no Node.js required
- 🦀 **Pure Rust** — Zero C dependencies. `boa_engine` for JS (no V8). Single static binary. Memory-safe.
- 🔌 **CDP Compatible** — Puppeteer, Playwright, and any Chrome DevTools Protocol client works out of the box
- 🛡️ **Secure by Default** — SSRF protection with CIDR blocking, `robots.txt` respect, no sandbox escape surface
- 📦 **Tiny Footprint** — 21 MB binary, ~8 MB base memory. Run 100 instances without breaking a sweat

---

## 🚀 Quick Start

### Install

**Cargo (all platforms)**

```bash
cargo install oxibrowser
```

**Build from source**

```bash
git clone https://github.com/a7garden/oxibrowser.git
cd oxibrowser
cargo build --release
# Binary at ./target/release/oxibrowser
```

**Use as a library**

```toml
# Cargo.toml
[dependencies]
oxibrowser-core = "0.7"
```

### Fetch a page

```bash
oxibrowser fetch https://example.com
```

### Start CDP server

```bash
oxibrowser serve --port 9222
```

Then connect with Puppeteer:

```javascript
import puppeteer from 'puppeteer-core';

const browser = await puppeteer.connect({
  browserWSEndpoint: 'ws://127.0.0.1:9222',
});

const page = await browser.newPage();
await page.goto('https://news.ycombinator.com');

// Get markdown — OxiBrowser's AI-native feature
const md = await page.evaluate(() => {
  // OXI domain available in evaluate context
});

console.log(await page.title());
await browser.close();
```

### Rust API

```rust
use oxibrowser_core::Browser;
use oxibrowser_core::config::BrowserConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let browser = Browser::new(BrowserConfig::default()).await?;
    let session = browser.new_session().await?;
    
    session.navigate("https://example.com").await?;
    
    let title = session.evaluate("document.title").await?;
    println!("Title: {:?}", title);
    
    Ok(())
}
```

---

## 🏗 Architecture

```
┌──────────────────────────────────────────────────────┐
│            Puppeteer / Playwright / Rust CDP          │
└────────────────────────┬─────────────────────────────┘
                         │ CDP WebSocket
                         ▼
┌──────────────────────────────────────────────────────┐
│                 CDP Server (10 domains)               │
│  Browser · DOM · Fetch · Input · Network             │
│  OXI · Page · Runtime · Target                       │
├──────────────────────────────────────────────────────┤
│          Browser → Session → Page → Frame            │
├──────────┬──────────┬──────────────┬─────────────────┤
│  WebAPI  │  Network │  JS Runtime  │  CSS Rendering  │
│  DOM     │  HTTP    │  boa_engine  │  PNG screenshot │
│  Tree    │  Cookies │  ES2024+     │  ASCII/Unicode  │
│  Storage │  SSRF    │  persistent  │  text→image     │
├──────────┴──────────┴──────────────┴─────────────────┤
│   html5ever · encoding_rs · reqwest · image · boa    │
└──────────────────────────────────────────────────────┘
```

### Crate Structure

| Crate | Lines | Purpose |
|-------|-------|---------|
| [`oxibrowser`](crates/oxibrowser/) | 1,233 | Binary + CLI (`fetch`, `serve`, `version`) |
| [`oxibrowser-core`](crates/oxibrowser-core/) | 12,953 | Browser engine: Session, Page, Frame, JS Runtime |
| [`oxibrowser-cdp`](crates/oxibrowser-cdp/) | 4,392 | CDP WebSocket server with 10 domain handlers |
| [`oxibrowser-webapi`](crates/oxibrowser-webapi/) | 1,549 | DOM tree, CSS selectors, Markdown conversion |
| **Total** | **20,127** | |

---

## 🌟 Features

### JavaScript Runtime (ES2024+)

Powered by [`boa_engine`](https://boajs.dev/) — pure Rust, no V8 dependency:

| Web API | Status |
|---------|--------|
| `document.querySelector` / `querySelectorAll` | ✅ Full |
| `document.createElement` / `createTextNode` | ✅ Full |
| `element.appendChild` / `removeChild` / `insertBefore` | ✅ Full |
| `element.getAttribute` / `setAttribute` / `removeAttribute` | ✅ Full |
| `element.cloneNode` / `remove()` | ✅ Full |
| `element.style` (CSSStyleDeclaration) | ✅ Property accessor |
| `element.classList` (DOMTokenList) | ✅ Property accessor |
| `element.textContent` / `innerHTML` | ✅ Read/Write |
| `element.addEventListener` / `dispatchEvent` | ✅ Full |
| `element.click()` | ✅ With event handlers |
| `fetch()` | ✅ Full (channel bridge) |
| `XMLHttpRequest` | ✅ Full with callbacks |
| `localStorage` | ✅ Persistent |
| `MutationObserver` | ✅ observe/disconnect/takeRecords |
| `setTimeout` / `setInterval` | ✅ TokioJobQueue |
| `console.log/warn/error` | ✅ With formatting |
| `URL` / `URLSearchParams` | ✅ Full |
| `crypto.getRandomValues` | ✅ Pseudo-random |
| `TextEncoder` / `TextDecoder` | ✅ UTF-8 |
| `atob` / `btoa` | ✅ Base64 |
| `requestAnimationFrame` | ✅ Polyfill |

### CDP Protocol (Chrome DevTools Protocol)

10 domain handlers — Puppeteer and Playwright compatible:

| Domain | Key Methods |
|--------|------------|
| **Browser** | `getVersion`, `close` |
| **DOM** | `getDocument`, `describeNode`, `querySelector`, `querySelectorAll` |
| **Fetch** | `enable/disable`, `continueRequest`, `failRequest`, `fulfillRequest`, `getResponseBody` |
| **Input** | `dispatchKeyEvent`, `dispatchMouseEvent`, `insertText` |
| **Network** | `enable/disable`, `setExtraHTTPHeaders`, `getResponseBody` |
| **OXI** 🤖 | `getMarkdown`, `getPageInfo` — AI-native extensions |
| **Page** | `navigate`, `captureScreenshot`, `getFrameTree`, `getTitle` |
| **Runtime** | `evaluate`, `callFunctionOn`, `enable`, `consoleAPICalled` |
| **Target** | `getTargets`, `attachToTarget`, `detachFromTarget` |

### OXI Domain — Built for AI Agents

The `OXI` CDP domain provides AI-optimized APIs that no other browser offers:

```python
import websockets, json, asyncio

async def ai_scrape():
    ws = await websockets.connect('ws://localhost:9222/ws')
    
    # Navigate
    await ws.send(json.dumps({
        "id": 1, "method": "Page.navigate",
        "params": {"url": "https://news.ycombinator.com"}
    }))
    await asyncio.sleep(2)
    
    # Get clean markdown — perfect for LLM ingestion
    await ws.send(json.dumps({
        "id": 2, "method": "OXI.getMarkdown"
    }))
    resp = json.loads(await ws.recv())
    print(resp['result']['markdown'])  # Clean markdown output
    
    # Get structured page info
    await ws.send(json.dumps({
        "id": 3, "method": "OXI.getPageInfo"
    }))
    info = json.loads(await ws.recv())
    print(info['result'])  # title, url, status, content-type, etc.
```

### Network Layer

| Feature | Description |
|---------|-------------|
| **HTTP Client** | `reqwest` with cookie persistence, redirect following |
| **Cookie Jar** | Domain-scoped cookie storage with `Set-Cookie` parsing |
| **SSRF Protection** | CIDR blocking for private network ranges |
| **robots.txt** | RFC 9309 compliant parser, `--obey-robots` flag |
| **Network Interception** | Pause, modify, or block any request via Fetch domain |
| **Custom Headers** | Per-session and per-request header injection |
| **Charset Detection** | `encoding_rs` for automatic charset detection and conversion |

### CSS Text Rendering

- **ASCII/Unicode text output** — Render DOM to readable text with proper indentation
- **Markdown conversion** — Full HTML→Markdown with heading, link, and list support
- **PNG screenshots** — Built-in 8×16 bitmap font, renders text content as images
- **No external dependencies** — Font data embedded in binary

---

## 🧪 Testing

```bash
# Run all 279 tests
cargo test --workspace

# E2E CDP tests (23 tests with real WebSocket)
cargo test -p oxibrowser-cdp

# Integration tests (real websites, --ignored)
cargo test --workspace -- --ignored

# Puppeteer smoke tests
cargo test -p oxibrowser --test smoke
```

---

## 📋 CLI Reference

```
oxibrowser 0.7.0
Headless browser with CDP support

USAGE:
    oxibrowser <COMMAND>

COMMANDS:
    fetch     Fetch and render a URL
    serve     Start CDP WebSocket server
    version   Print version information

FETCH OPTIONS:
    <URL>                  URL to fetch
    --dump <FORMAT>        Output format: text, html, markdown [default: text]

SERVE OPTIONS:
    --host <HOST>          Bind address [default: 127.0.0.1]
    --port <PORT>          Bind port [default: 9222]
    --obey-robots          Respect robots.txt
    --log-level <LEVEL>    Log level: trace, debug, info, warn, error [default: info]
```

---

## 🔧 Advanced Usage

### Custom Browser Configuration

```rust
use oxibrowser_core::{Browser, config::BrowserConfig};
use std::time::Duration;

let config = BrowserConfig {
    user_agent: "MyBot/1.0".to_string(),
    timeout: Duration::from_secs(30),
    obey_robots: true,
    max_redirects: 10,
    ..Default::default()
};

let browser = Browser::new(config).await?;
```

### Request Interception

```javascript
// With Puppeteer
const client = await page.target().createCDPSession();

await client.send('Fetch.enable', {
    patterns: [{ urlPattern: '*ads*' }]
});

client.on('Fetch.requestPaused', async ({ requestId }) => {
    // Block ad requests
    await client.send('Fetch.failRequest', {
        requestId,
        reason: 'BlockedByClient'
    });
});
```

### Screenshot Capture

```javascript
// PNG screenshot via CDP
const { data } = await client.send('Page.captureScreenshot', {
    format: 'png'
});
// data is base64-encoded PNG
```

---

## 🗺 Roadmap

### v0.7.0 (Current) ✅
- [x] Mutation persistence across `evaluate()` calls
- [x] `style` / `classList` as property accessors
- [x] `textContent` / `innerHTML` read/write
- [x] DOM mutation: `createElement`, `appendChild`, `removeChild`, `insertBefore`

### v0.8.0 (Next)
- [ ] Event bubbling and propagation
- [ ] Full `textContent` with subtree text collection
- [ ] HTML parsing in `innerHTML` setter
- [ ] `window.location` setter (navigation)

### v0.9.0
- [ ] Canvas 2D API (basic drawing operations)
- [ ] iframe support (nested frames)
- [ ] `window.history` / back-forward navigation
- [ ] WebSocket API in JS runtime

### v1.0.0
- [ ] CSS layout engine (box model, positioning)
- [ ] WebGL support (basic)
- [ ] Service Worker stubs
- [ ] Multi-process isolation

See [HEADLESS_ROADMAP.md](docs/HEADLESS_ROADMAP.md) for detailed planning.

---

## 🤝 Contributing

Contributions are welcome! Whether it's:

- 🐛 **Bug reports** — [Open an issue](https://github.com/a7garden/oxibrowser/issues)
- 💡 **Feature requests** — [Start a discussion](https://github.com/a7garden/oxibrowser/issues)
- 🔧 **Pull requests** — Fork, branch, PR. All PRs need passing tests.
- 📖 **Documentation** — Fix typos, add examples, improve guides

### Development Setup

```bash
git clone https://github.com/a7garden/oxibrowser.git
cd oxibrowser
cargo build
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

---

## 📄 License

OxiBrowser is licensed under the [MIT License](LICENSE).

---

## 🙏 Acknowledgments

- [Lightpanda](https://github.com/lightpanda-io/browser) — Architecture inspiration (Browser → Session → Page → Frame hierarchy)
- [boa_engine](https://boajs.dev/) — Pure Rust JavaScript engine (ES2024+)
- [html5ever](https://github.com/servo/html5ever) — HTML parser from the Servo project
- [reqwest](https://github.com/seanmonstar/reqwest) — Ergonomic HTTP client for Rust
- [tokio](https://tokio.rs/) — Async runtime powering the entire networking stack

---

<div align="center">

**[⬆ Back to Top](#-oxibrowser)**

Made with 🦀 in Rust

</div>
