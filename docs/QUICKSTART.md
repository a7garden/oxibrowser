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
oxibrowser-core = "0.11"
# Or the full CDP server:
oxibrowser-cdp = "0.11"
```

## CLI Usage

OxiBrowser has 8 subcommands designed for both humans and AI agents.
Human-readable output by default; add `--json` for machine-readable output.

### fetch — Fetch and render a page

```bash
# Default: markdown output (human-readable)
oxibrowser fetch https://example.com

# Agent mode: JSON output
oxibrowser fetch https://example.com --json

# Text format
oxibrowser fetch https://example.com --format text

# Quick summary (title, links, headings)
oxibrowser fetch https://example.com --summary

# Summary in JSON
oxibrowser fetch https://example.com --summary --json

# Click an element, wait, then read
oxibrowser fetch https://example.com --click button --wait .result --json

# Evaluate JavaScript
oxibrowser fetch https://example.com --eval "document.title" --json

# Limit response size (for agents)
oxibrowser fetch https://example.com --max-bytes 8000 --json

# Select specific fields
oxibrowser fetch https://example.com --fields url,title,status --json
```

### extract — Extract structured data

```bash
# Get all links
oxibrowser extract https://example.com --links --json

# Title + links (human-readable)
oxibrowser extract https://example.com --title --links

# Extract elements by CSS selector
oxibrowser extract https://example.com --selector "a" --all --attrs text,href --json

# Full page text
oxibrowser extract https://example.com --text --json

# Markdown content
oxibrowser extract https://example.com --markdown --json
```

### session — Interactive JSON REPL

Start a session for multi-step browser automation:

```bash
oxibrowser session
```

Then type commands (one per line). Each command produces a JSON response:

```
new                                    # Create tab → {"ok":true,"data":{"tab_id":"t1"}}
goto t1 https://example.com            # Navigate → {"ok":true,"data":{"status":200}}
eval t1 document.title                 # Run JS   → {"ok":true,"data":{"value":"Example Domain"}}
click t1 a                             # Click    → {"ok":true}
content t1 --format markdown           # Read     → {"ok":true,"data":{"markdown":"..."}}
close t1                               # Close    → {"ok":true,"data":{"closed":"t1"}}
exit                                   # Quit     → {"ok":true,"data":{"exit":true}}
```

**22 session commands**: `new`, `goto`, `back`, `forward`, `reload`, `click`, `fill`,
`press`, `type`, `select`, `check`, `uncheck`, `scroll`, `eval`, `extract`, `content`,
`screenshot`, `wait`, `close`, `close --all`, `list`, `help`, `exit`

Clean shutdown on EOF (stdin close), `exit` command, Ctrl+C, or SIGTERM.

### run — YAML automation

```yaml
name: example
steps:
  - step_type: goto
    data:
      goto: https://example.com
  - step_type: content
    data:
      format: markdown
```

```bash
oxibrowser run script.yaml
# {"ok":true,"data":{"success":true,"name":"example","steps":[...]}, ...}
```

### serve — CDP server

```bash
# Default: localhost:9222
oxibrowser serve

# Custom host/port
oxibrowser serve --host 0.0.0.0 --port 8080

# Cookie persistence
oxibrowser serve --cookie-file cookies.json
```

### describe — CLI schema (for agents)

```bash
# Compact: all commands (~200 tokens)
oxibrowser describe --compact

# Full details for one command
oxibrowser describe fetch
oxibrowser describe session
```

### skill — Agent skill guide

```bash
# Markdown (for prompt injection)
oxibrowser skill

# JSON (wrapped in CliResponse)
oxibrowser skill --json
```

### version

```bash
oxibrowser version          # "oxibrowser 0.11.0"
oxibrowser version --json   # {"ok":true,"data":{"version":"0.11.0","name":"oxibrowser"}}
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
    
    await cdp("Page.navigate", {"url": "https://example.com"})
    await asyncio.sleep(2)
    
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
    
    session.navigate("https://example.com").await?;
    
    let title = session.evaluate("document.title").await?;
    println!("Title: {:?}", title);
    
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

```json
{"id": 1, "method": "OXI.getMarkdown"}
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

```json
{"id": 2, "method": "OXI.getPageInfo"}
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

## JSON Output Format

All `--json` responses use the same schema:

```json
{
  "ok": true,
  "data": { ... },
  "meta": { "elapsed_ms": 152 }
}
```

On error:

```json
{
  "ok": false,
  "error": "URL scheme must be http or https",
  "error_code": "INVALID_URL"
}
```

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Runtime error |
| 2 | Input validation error |
| 3 | Timeout |
| 4 | Network error |

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
console.log(found.textContent);

// Style manipulation
found.style.setProperty('color', 'red');

// Class manipulation
found.classList.add('visible');
console.log(found.classList.contains('active'));

// Fetch API
const resp = await fetch('https://httpbin.org/json');
const data = await resp.json();

// Events
document.querySelector('button').addEventListener('click', (e) => {
    console.log('Clicked!', e.type);
});

// localStorage
localStorage.setItem('key', 'value');
console.log(localStorage.getItem('key'));
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
