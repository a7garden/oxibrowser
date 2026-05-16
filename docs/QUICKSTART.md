# OxiBrowser Quick Start Guide

## Installation

### From crates.io

```bash
cargo install oxibrowser
```

### From source

```bash
git clone https://github.com/a7garden/oxibrowser.git
cd oxibrowser
cargo build --release
# Binary: ./target/release/oxibrowser
```

### As a Rust dependency

```toml
[dependencies]
oxibrowser-core = "0.7"
# Or the full CDP server:
oxibrowser-cdp = "0.7"
```

## CLI Usage

### Fetch a page

```bash
# Render as text (default)
oxibrowser fetch https://example.com

# Output as markdown
oxibrowser fetch --dump markdown https://example.com

# Output raw HTML
oxibrowser fetch --dump html https://example.com
```

### Start CDP server

```bash
# Default: localhost:9222
oxibrowser serve

# Custom host/port
oxibrowser serve --host 0.0.0.0 --port 8080

# Respect robots.txt
oxibrowser serve --obey-robots

# With debug logging
oxibrowser serve --log-level debug
```

### Version info

```bash
oxibrowser version
```

## Using with Puppeteer

```javascript
import puppeteer from 'puppeteer-core';

const browser = await puppeteer.connect({
    browserWSEndpoint: 'ws://127.0.0.1:9222',
});

const page = await browser.newPage();
await page.goto('https://example.com');

const title = await page.title();
console.log('Title:', title);

const content = await page.evaluate(() => document.body.innerHTML);
console.log('Content:', content);

await browser.close();
```

## Using with Playwright

```javascript
import { chromium } from 'playwright-core';

const browser = await chromium.connectOverCDP('ws://127.0.0.1:9222');
const context = await browser.newContext();
const page = await context.newPage();

await page.goto('https://example.com');
console.log(await page.title());

await browser.close();
```

## Using with Python (websockets)

```python
import asyncio, json, websockets

async def browse():
    ws = await websockets.connect('ws://localhost:9222/ws')
    msg_id = 1
    
    async def cdp(method, params=None):
        nonlocal msg_id
        req = {"id": msg_id, "method": method}
        if params:
            req["params"] = params
        await ws.send(json.dumps(req))
        while True:
            r = json.loads(await ws.recv())
            if 'id' in r and r['id'] == msg_id:
                msg_id += 1
                return r
    
    # Navigate
    await cdp("Page.navigate", {"url": "https://example.com"})
    await asyncio.sleep(2)
    
    # Evaluate JS
    resp = await cdp("Runtime.evaluate", {
        "expression": "document.title",
        "returnByValue": True
    })
    print("Title:", resp['result']['result']['value'])
    
    # Get markdown (OXI domain)
    resp = await cdp("OXI.getMarkdown")
    print(resp['result']['markdown'][:500])
    
    await ws.close()

asyncio.run(browse())
```

## Rust API

### Basic browsing

```rust
use oxibrowser_core::{Browser, config::BrowserConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let browser = Browser::new(BrowserConfig::default()).await?;
    let session = browser.new_session().await?;
    
    // Navigate
    session.navigate("https://example.com").await?;
    
    // Evaluate JavaScript
    let result = session.evaluate("document.title").await?;
    println!("Title: {:?}", result);
    
    // Get page info
    let url = session.current_url();
    println!("URL: {:?}", url);
    
    Ok(())
}
```

### CDP Server in Rust

```rust
use oxibrowser_cdp::server::CdpServer;
use oxibrowser_core::{Browser, config::BrowserConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let browser = Browser::new(BrowserConfig::default()).await?;
    let server = CdpServer::new(browser, "127.0.0.1:9222");
    
    println!("CDP server on ws://127.0.0.1:9222/ws");
    server.run().await?;
    
    Ok(())
}
```

## OXI Domain — AI Agent API

The `OXI` CDP domain is OxiBrowser's unique feature for AI workflows:

### Get Markdown

Returns clean markdown from the current page — perfect for LLM ingestion:

```json
{
    "id": 1,
    "method": "OXI.getMarkdown"
}
```

Response:
```json
{
    "id": 1,
    "result": {
        "markdown": "# Example Domain\n\nThis domain is for use in illustrative examples..."
    }
}
```

### Get Page Info

Returns structured page metadata:

```json
{
    "id": 2,
    "method": "OXI.getPageInfo"
}
```

Response:
```json
{
    "id": 2,
    "result": {
        "url": "https://example.com",
        "title": "Example Domain",
        "statusCode": 200,
        "contentType": "text/html",
        "contentLength": 1256
    }
}
```

## JavaScript API Support

OxiBrowser supports a wide range of JavaScript Web APIs:

```javascript
// DOM manipulation
const el = document.createElement('div');
el.setAttribute('id', 'test');
el.textContent = 'Hello, World!';
document.body.appendChild(el);

// Query elements
const found = document.querySelector('#test');
console.log(found.textContent);  // "Hello, World!"

// Style manipulation
found.style.setProperty('color', 'red');
console.log(found.style.getPropertyValue('color'));  // "red"

// Class manipulation
found.setAttribute('class', 'active highlight');
found.classList.add('visible');
console.log(found.classList.contains('active'));  // true

// Fetch API
const resp = await fetch('https://httpbin.org/json');
const data = await resp.json();
console.log(data);

// Events
document.querySelector('button').addEventListener('click', (e) => {
    console.log('Clicked!', e.type);
});

// localStorage
localStorage.setItem('key', 'value');
console.log(localStorage.getItem('key'));  // "value"

// Timers
setTimeout(() => console.log('1 second later'), 1000);
```

## Screenshots

Capture PNG screenshots of rendered page content:

```json
{
    "id": 1,
    "method": "Page.captureScreenshot",
    "params": {"format": "png"}
}
```

The response contains a base64-encoded PNG image using OxiBrowser's built-in
8×16 bitmap font for text rendering.

## Network Interception

Block, modify, or respond to requests programmatically:

```javascript
const client = await page.target().createCDPSession();

// Enable interception
await client.send('Fetch.enable', {
    patterns: [{ urlPattern: '*' }]
});

// Handle paused requests
client.on('Fetch.requestPaused', async ({ requestId, request }) => {
    if (request.url.includes('ads')) {
        await client.send('Fetch.failRequest', {
            requestId,
            reason: 'BlockedByClient'
        });
    } else {
        await client.send('Fetch.continueRequest', { requestId });
    }
});
```

## Configuration

### BrowserConfig

```rust
use oxibrowser_core::config::BrowserConfig;
use std::time::Duration;

let config = BrowserConfig {
    user_agent: "MyBot/1.0".to_string(),
    timeout: Duration::from_secs(30),
    obey_robots: true,
    max_redirects: 10,
    ..Default::default()
};
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level (`trace`, `debug`, `info`, `warn`, `error`) |

## Limitations

OxiBrowser is a headless browser focused on content extraction and automation.
It does NOT support:

- Visual/CSS layout rendering (no box model, no computed styles)
- JavaScript-driven navigation (`window.location = url` — use CDP `Page.navigate` instead)
- WebSocket API in JS runtime (CDP WebSocket works fine)
- Canvas 2D / WebGL
- Audio / Video playback
- Browser extensions
- Multi-process isolation

See [HEADLESS_ROADMAP.md](HEADLESS_ROADMAP.md) for planned features.
