# Chrome DevTools Protocol (CDP) Implementation Guide

## Overview

OxiBrowser implements the Chrome DevTools Protocol (CDP) version 1.3, enabling compatibility with automation tools like Puppeteer, Playwright, and Chrome DevTools.

The CDP implementation lives in the `oxibrowser-cdp` crate and consists of:
- **HTTP endpoints** for browser/target discovery
- **WebSocket handler** for bidirectional CDP communication
- **Domain dispatch** routing `Domain.method` strings to handler functions

## CDP Domains

OxiBrowser implements six core CDP domains:

| Domain | Status | Purpose |
|--------|--------|---------|
| `Browser` | Planned | Browser metadata and version info |
| `DOM` | Planned | DOM tree inspection and manipulation |
| `Network` | Planned | Network request/response monitoring and interception |
| `Page` | Planned | Page navigation, lifecycle events, screenshots |
| `Runtime` | Planned | JavaScript evaluation and object inspection |
| `Target` | Planned | Target discovery and session management |

---

## Browser Domain

Browser-wide operations and metadata.

### Methods

| Method | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `Browser.getVersion` | *(none)* | `{ protocolVersion, product, revision, userAgent, jsVersion }` | Returns browser version information |
| `Browser.getWindowBounds` | `{ windowId }` | `{ bounds: { left, top, width, height, windowState } }` | Get window bounds |
| `Browser.setWindowBounds` | `{ windowId, bounds }` | *(none)* | Set window bounds |
| `Browser.getWindowForTarget` | `{ targetId }` | `{ windowId, bounds }` | Get window for a target |
| `Browser.close` | *(none)* | *(none)* | Close the browser |

---

## DOM Domain

DOM tree inspection and manipulation. Maps directly to `oxibrowser-webapi` types.

### Methods

| Method | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `DOM.getDocument` | `{ depth, pierce }` | `{ root: Node }` | Returns the root DOM node |
| `DOM.describeNode` | `{ nodeId, backendNodeId, objectId, depth, pierce }` | `{ node }` | Describes a DOM node |
| `DOM.querySelector` | `{ nodeId, selector }` | `{ nodeId }` | Find first matching element |
| `DOM.querySelectorAll` | `{ nodeId, selector }` | `{ nodeIds }` | Find all matching elements |
| `DOM.getNodeForLocation` | `{ x, y, includeUserAgentShadowDOM }` | `{ backendNodeId, frameId }` | Get node at coordinates |
| `DOM.removeChild` | `{ parentNodeId, childNodeId }` | *(none)* | Remove a child node |
| `DOM.setAttributeValue` | `{ nodeId, name, value }` | *(none)* | Set element attribute |
| `DOM.getAttributes` | `{ nodeId }` | `{ attributes: [name, value, ...] }` | Get element attributes |
| `DOM.getOuterHTML` | `{ nodeId, backendNodeId, objectId }` | `{ outerHTML }` | Get node's outer HTML |
| `DOM.setOuterHTML` | `{ nodeId, outerHTML }` | *(none)* | Set node's outer HTML |
| `DOM.focus` | `{ nodeId, backendNodeId, objectId }` | *(none)* | Focus a node |

### DOM Node Wire Format

```json
{
  "nodeId": 1,
  "backendNodeId": 1,
  "nodeType": 1,
  "nodeName": "DIV",
  "localName": "div",
  "nodeValue": "",
  "childNodeCount": 3,
  "attributes": ["class", "container", "id", "main"],
  "children": [...]
}
```

### NodeType Mapping

| DOM `NodeType` variant | CDP `nodeType` |
|------------------------|---------------|
| `Document` | 9 |
| `Element { tag, attributes }` | 1 |
| `Text(text)` | 3 |
| `Comment(text)` | 8 |
| `Doctype { name }` | 10 |

---

## Network Domain

Network request/response monitoring, cookie management, and caching.

### Methods

| Method | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `Network.enable` | `{ maxTotalBufferSize, maxResourceBufferSize, maxPostDataSize }` | *(none)* | Enable network events |
| `Network.disable` | *(none)* | *(none)* | Disable network events |
| `Network.getAllCookies` | *(none)* | `{ cookies }` | Get all browser cookies |
| `Network.getCookies` | `{ urls }` | `{ cookies }` | Get cookies for URLs |
| `Network.setCookie` | `{ name, value, url, domain, path, secure, httpOnly, sameSite, expires, priority, sameParty, sourceScheme }` | `{ success }` | Set a cookie |
| `Network.deleteCookies` | `{ name, url, domain, path }` | *(none)* | Delete cookies |
| `Network.setExtraHTTPHeaders` | `{ headers }` | *(none)* | Set extra HTTP headers |
| `Network.emulateNetworkConditions` | `{ offline, latency, downloadThroughput, uploadThroughput }` | *(none)* | Emulate network conditions |

### Events

| Event | Parameters | Description |
|-------|-----------|-------------|
| `Network.requestWillBeSent` | `{ requestId, frameId, loaderId, documentURL, request, timestamp, ... }` | Fired when a request is about to be sent |
| `Network.responseReceived` | `{ requestId, frameId, loaderId, type, response, timestamp }` | Fired when response is received |
| `Network.loadingFinished` | `{ requestId, timestamp, encodedDataLength }` | Fired when loading finishes |
| `Network.loadingFailed` | `{ requestId, timestamp, type, errorText, canceled }` | Fired when loading fails |

### Cookie Wire Format

```json
{
  "name": "session",
  "value": "abc123",
  "domain": ".example.com",
  "path": "/",
  "expires": 1234567890,
  "size": 42,
  "httpOnly": false,
  "secure": true,
  "sameSite": "Lax",
  "priority": "Medium",
  "sameParty": false,
  "sourceScheme": "Secure"
}
```

---

## Page Domain

Page navigation, lifecycle events, frame tree, and screenshots.

### Methods

| Method | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `Page.navigate` | `{ url, referrer, transitionType, frameId }` | `{ frameId, loaderId, errorText }` | Navigate to URL |
| `Page.reload` | `{ ignoreCache, scriptToEvaluateOnNewDocument }` | *(none)* | Reload current page |
| `Page.getFrameTree` | *(none)* | `{ frameTree }` | Get frame tree |
| `Page.getFrameHierarchy` | *(none)* | `{ frameTree }` | Get frame hierarchy |
| `Page.enable` | *(none)* | *(none)* | Enable page events |
| `Page.disable` | *(none)* | *(none)* | Disable page events |
| `Page.close` | *(none)* | *(none)* | Close the page |
| `Page.captureScreenshot` | `{ format, quality, clip, fromSurface, captureBeyondViewport }` | `{ data, metadata }` | Take a screenshot |
| `Page.printToPDF` | `{ landscape, displayHeaderFooter, printBackground, scale, paperWidth, paperHeight, marginTop, marginBottom, marginLeft, marginRight }` | `{ data, stream }` | Print page to PDF |
| `Page.addScriptToEvaluateOnNewDocument` | `{ source, worldName, includeCommandLineAPI }` | `{ identifier }` | Add script to run on each document load |
| `Page.removeScriptToEvaluateOnNewDocument` | `{ identifier }` | *(none)* | Remove auto-evaluate script |

### Events

| Event | Parameters | Description |
|-------|-----------|-------------|
| `Page.frameNavigated` | `{ frame }` | Fired when frame navigates |
| `Page.loadEventFired` | `{ timestamp }` | Fired when page load event fires |
| `Page.domContentEventFired` | `{ timestamp }` | Fired when DOMContentLoaded fires |
| `Page.frameAttached` | `{ frameId, parentFrameId }` | Fired when frame is attached |
| `Page.frameDetached` | `{ frameId, reason }` | Fired when frame is detached |

### Frame Tree Wire Format

```json
{
  "frameTree": {
    "frame": {
      "id": "frame-1",
      "url": "https://example.com",
      "loaderId": "loader-1",
      "securityOrigin": "https://example.com",
      "mimeType": "text/html"
    },
    "childFrames": [
      {
        "frame": {
          "id": "frame-2",
          "url": "https://example.com/iframe",
          "parentId": "frame-1"
        }
      }
    ]
  }
}
```

---

## Runtime Domain

JavaScript evaluation, object inspection, and execution context management.

### Methods

| Method | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `Runtime.enable` | *(none)* | *(none)* | Enable runtime events |
| `Runtime.disable` | *(none)* | *(none)* | Disable runtime events |
| `Runtime.evaluate` | `{ expression, objectGroup, includeCommandLineAPI, silent, returnByValue, awaitPromise, ... }` | `{ result, exceptionDetails }` | Evaluate JavaScript expression |
| `Runtime.callFunctionOn` | `{ functionDeclaration, objectId, arguments, returnByValue, awaitPromise, ... }` | `{ result, exceptionDetails }` | Call function on remote object |
| `Runtime.getProperties` | `{ objectId, ownProperties, accessorPropertiesOnly, generatePreview }` | `{ result, internalProperties }` | Get object properties |
| `Runtime.releaseObject` | `{ objectId }` | *(none)* | Release remote object |
| `Runtime.releaseObjectGroup` | `{ objectGroup }` | *(none)* | Release object group |

### RemoteObject Wire Format

```json
{
  "type": "string",
  "subtype": null,
  "className": null,
  "value": "Hello, World!",
  "description": "Hello, World!",
  "objectId": null,
  "unserializableValue": null,
  "preview": null
}
```

### RemoteObjectType Mapping

| JsEvalResult value | CDP `type` | CDP `subtype` |
|--------------------|-----------|---------------|
| `Value::String(s)` | `"string"` | null |
| `Value::Number(n)` | `"number"` | null |
| `Value::Bool(b)` | `"boolean"` | null |
| `Value::Null` | `"null"` | null |
| `Value::Object(o)` | `"object"` | null or `"array"` |
| Exception | `"string"` | `"error"` |

---

## Target Domain

Target (page/frame) discovery and session attachment.

### Methods

| Method | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `Target.setDiscoverTargets` | `{ discover, filter }` | *(none)* | Enable/disable target discovery |
| `Target.setAutoAttach` | `{ autoAttach, waitForDebuggerOnStart, flatten, filter }` | *(none)* | Auto-attach to new targets |
| `Target.attachToTarget` | `{ targetId, flatten }` | `{ sessionId }` | Attach to a target |
| `Target.detachFromTarget` | `{ sessionId, targetId }` | *(none)* | Detach from target |
| `Target.createTarget` | `{ url, width, height, newWindow, background }` | `{ targetId }` | Create a new target (page) |
| `Target.closeTarget` | `{ targetId }` | `{ success }` | Close a target |
| `Target.getTargets` | `{ filter }` | `{ targetInfos }` | Get list of targets |

### Events

| Event | Parameters | Description |
|-------|-----------|-------------|
| `Target.targetCreated` | `{ targetInfo }` | Fired when a target is created |
| `Target.targetDestroyed` | `{ targetId }` | Fired when a target is destroyed |
| `Target.targetInfoChanged` | `{ targetInfo }` | Fired when target info changes |
| `Target.attachedToTarget` | `{ sessionId, targetInfo, waitingForDebugger }` | Fired when attached to target |
| `Target.detachedFromTarget` | `{ sessionId, targetId }` | Fired when detached from target |

### TargetInfo Wire Format

```json
{
  "targetId": "page-1",
  "type": "page",
  "title": "Example Page",
  "url": "https://example.com",
  "attached": true,
  "openerId": null,
  "canAccessOpener": false,
  "browserContextId": "session-1"
}
```

---

## HTTP Endpoints

### GET /json/version

Returns browser metadata for tool discovery.

**Response:**
```json
{
  "browser": "OxiBrowser/0.1.0",
  "protocolVersion": "1.3",
  "userAgent": "OxiBrowser/0.1.0",
  "v8Version": "0.1.0",
  "webkitVersion": "0.1.0",
  "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser/browser-1"
}
```

Maps to `JsonVersion` struct in `protocol.rs`.

### GET /json

Returns a list of debuggable page targets.

**Response:**
```json
[
  {
    "id": "page-1",
    "title": "Example Page",
    "type": "page",
    "url": "https://example.com",
    "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/page-1"
  }
]
```

Maps to `Vec<JsonTarget>` in `protocol.rs`.

---

## WebSocket Message Format

### Full Exchange Example

```javascript
// 1. Connect
const ws = new WebSocket("ws://127.0.0.1:9222/devtools/browser/browser-1");

// 2. Enable page events
ws.send(JSON.stringify({ id: 1, method: "Page.enable" }));
// Response: {"id":1,"result":{}}

// 3. Navigate
ws.send(JSON.stringify({
  id: 2,
  method: "Page.navigate",
  params: { url: "https://example.com" }
}));
// Response: {"id":2,"result":{"frameId":"frame-1","loaderId":"loader-1"}}

// 4. Receive events (unsolicited)
// Event: {"method":"Page.frameNavigated","params":{"frame":{"id":"frame-1","url":"https://example.com"}}}
// Event: {"method":"Page.loadEventFired","params":{"timestamp":1234567890.0}}

// 5. Evaluate JS
ws.send(JSON.stringify({
  id: 3,
  method: "Runtime.evaluate",
  params: { expression: "document.title" }
}));
// Response: {"id":3,"result":{"result":{"type":"string","value":"Example Domain"}}}

// 6. Get DOM document
ws.send(JSON.stringify({ id: 4, method: "DOM.getDocument" }));
// Response: {"id":4,"result":{"root":{"nodeId":1,"nodeType":9,"nodeName":"#document",...}}}
```

---

## Puppeteer / Playwright Compatibility Matrix

### Puppeteer Operations → CDP Methods

| Puppeteer Operation | CDP Method(s) |
|---------------------|---------------|
| `browser.newPage()` | `Target.createTarget` |
| `page.goto(url)` | `Page.navigate` |
| `page.goBack()` | *(manual: Page.navigate to previous URL)* |
| `page.goForward()` | *(manual: Page.navigate to next URL)* |
| `page.reload()` | `Page.reload` |
| `page.close()` | `Page.close` or `Target.closeTarget` |
| `page.title()` | `Runtime.evaluate("document.title")` |
| `page.url()` | `Runtime.evaluate("document.URL")` |
| `page.content()` | `Runtime.evaluate("document.documentElement.outerHTML")` |
| `page.evaluate(expr)` | `Runtime.evaluate` |
| `page.$(selector)` | `DOM.querySelector` |
| `page.$$(selector)` | `DOM.querySelectorAll` |
| `page.screenshot()` | `Page.captureScreenshot` |
| `page.pdf()` | `Page.printToPDF` |
| `page.cookies()` | `Network.getCookies` |
| `page.setCookie(...)` | `Network.setCookie` |
| `page.on('response', ...)` | `Network.enable` + `Network.responseReceived` |
| `page.on('request', ...)` | `Network.enable` + `Network.requestWillBeSent` |
| `page.setViewport(...)` | `Browser.setWindowBounds` or `Emulation.setDeviceMetricsOverride` |
| `browser.version()` | `Browser.getVersion` |
| `browser.disconnect()` | Close WebSocket |
| `browser.close()` | `Browser.close` |

### Playwright Operations → CDP Methods

| Playwright Operation | CDP Method(s) |
|---------------------|---------------|
| `browser.newPage()` | `Target.createTarget` |
| `page.goto(url)` | `Page.navigate` |
| `page.evaluate(expr)` | `Runtime.evaluate` |
| `page.locator(selector)` | `DOM.querySelector` + `DOM.describeNode` |
| `page.content()` | `Runtime.evaluate("document.documentElement.outerHTML")` |
| `page.screenshot()` | `Page.captureScreenshot` |
| `page.context().cookies()` | `Network.getCookies` |
| `page.route(url, handler)` | `Network.enable` + `Fetch.enable` (interception) |
| `browser.newContext()` | Create new OxiBrowser Session |
| `context.newPage()` | `Target.createTarget` in session |

### Current Compatibility Status

| Operation | Status |
|-----------|--------|
| Browser discovery (`/json/version`) | ✅ Protocol types defined |
| Target listing (`/json`) | ✅ Protocol types defined |
| WebSocket connection | 🔲 Server not yet implemented |
| Navigation (`Page.navigate`) | 🔲 Domain handler not yet implemented |
| JS evaluation (`Runtime.evaluate`) | 🔲 Domain handler not yet implemented |
| DOM inspection (`DOM.getDocument`) | 🔲 Domain handler not yet implemented |
| Screenshot (`Page.captureScreenshot`) | 🔲 Domain handler not yet implemented |
| Network monitoring | 🔲 Domain handler not yet implemented |

---

## Implementation Notes

### Dispatch Architecture

All domain methods follow the same pattern:

```rust
pub fn handle(method: &str, params: Option<Value>) -> DomainResult {
    match method {
        "methodName" => method_name(params),
        _ => Err(CdpError {
            code: -32601,
            message: format!("unknown method: DomainName.{}", method),
        }),
    }
}
```

`DomainResult` is `Result<Option<serde_json::Value>, CdpError>`:
- `Ok(Some(value))` → successful response with result data
- `Ok(None)` → successful response with empty result (`{}`)
- `Err(CdpError)` → error response with code and message

### Adding a New Method

1. Add a `match` arm in the domain's `handle()` function
2. Implement the handler function
3. Map to core types (e.g., `Document::query_selector` for `DOM.querySelector`)
4. Return `serde_json::json!({...})` for the result
5. Add tests

### Event Emission

Events are sent asynchronously from the server to the client:

```rust
CdpEvent::new("Page.loadEventFired", json!({ "timestamp": ts }))
```

The CDP server maintains an event channel per CdpSession. Domain handlers can access it to push events.

### Session Multiplexing

CDP supports multiplexing multiple target sessions over a single WebSocket connection using `sessionId`:

```json
// Attach to a target
{ "id": 1, "method": "Target.attachToTarget", "params": { "targetId": "page-1" } }
// Response: { "id": 1, "result": { "sessionId": "sess-abc" } }

// Subsequent messages with sessionId
{ "id": 2, "method": "Page.navigate", "params": { "url": "..." }, "sessionId": "sess-abc" }
```
