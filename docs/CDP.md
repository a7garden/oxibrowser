# CDP Protocol Reference

OxiBrowser implements the Chrome DevTools Protocol (CDP) over WebSocket,
providing compatibility with Puppeteer, Playwright, and other CDP clients.

## Connection

### HTTP Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /json/version` | Browser version info |
| `GET /json` | Available targets |
| `GET /` | Simple HTML status page |

### WebSocket

Connect to `ws://host:port/ws` to send CDP commands.

## Protocol Format

### Request

```json
{
    "id": 1,
    "method": "Page.navigate",
    "params": {
        "url": "https://example.com"
    }
}
```

### Response

```json
{
    "id": 1,
    "result": {
        "frameId": "frame-1",
        "loaderId": "loader-1"
    }
}
```

### Error

```json
{
    "id": 1,
    "error": {
        "code": -32600,
        "message": "Invalid request"
    }
}
```

### Events

```json
{
    "method": "Page.frameNavigated",
    "params": {
        "frame": {
            "id": "frame-1",
            "url": "https://example.com"
        }
    }
}
```

## Domain Reference

### Browser

| Method | Parameters | Description |
|--------|-----------|-------------|
| `Browser.getVersion` | — | Returns browser version info |
| `Browser.close` | — | Closes the browser |

### DOM

| Method | Parameters | Description |
|--------|-----------|-------------|
| `DOM.getDocument` | `depth?` | Returns root DOM node |
| `DOM.describeNode` | `nodeId` | Returns node info |
| `DOM.querySelector` | `nodeId, selector` | Find first matching element |
| `DOM.querySelectorAll` | `nodeId, selector` | Find all matching elements |
| `DOM.getOuterHTML` | `nodeId` | Get outer HTML of node |
| `DOM.removeAttribute` | `nodeId, name` | Remove an attribute |
| `DOM.setNodeValue` | `nodeId, value` | Set node value |

### Fetch

| Method | Parameters | Description |
|--------|-----------|-------------|
| `Fetch.enable` | `patterns?, handleAuthRequests?` | Enable request interception |
| `Fetch.disable` | — | Disable interception |
| `Fetch.continueRequest` | `requestId, url?, headers?, postData?` | Continue with modifications |
| `Fetch.failRequest` | `requestId, reason` | Fail the request |
| `Fetch.fulfillRequest` | `requestId, responseCode, responseHeaders, body?` | Return synthetic response |
| `Fetch.getResponseBody` | `requestId` | Get response body |

**Interception flow:**

1. `Fetch.enable({ patterns: [...] })` — start intercepting
2. `Fetch.requestPaused` event fires for each matching request
3. Respond with one of:
   - `Fetch.continueRequest` — allow with optional modifications
   - `Fetch.failRequest` — block the request
   - `Fetch.fulfillRequest` — return a synthetic response

### Input

| Method | Parameters | Description |
|--------|-----------|-------------|
| `Input.dispatchKeyEvent` | `type, key, code, text?` | Dispatch keyboard event |
| `Input.dispatchMouseEvent` | `type, x, y, button?` | Dispatch mouse event |
| `Input.insertText` | `text` | Insert text at cursor |

Key event types: `keyDown`, `keyUp`, `rawKeyDown`, `char`

Mouse event types: `mousePressed`, `mouseReleased`, `mouseMoved`

### Network

| Method | Parameters | Description |
|--------|-----------|-------------|
| `Network.enable` | `maxTotalBufferSize?, maxResourceBufferSize?` | Enable network events |
| `Network.disable` | — | Disable network events |
| `Network.setExtraHTTPHeaders` | `headers` | Set default headers |
| `Network.getResponseBody` | `requestId` | Get response body for request |
| `Network.getAllCookies` | — | Get all cookies |
| `Network.getCookies` | `urls?` | Get cookies for URLs |
| `Network.setCookie` | `name, value, domain?, url?, ...` | Set a cookie |
| `Network.deleteCookies` | `name, domain?, url?` | Delete cookies |

**Events:**

| Event | Description |
|-------|-------------|
| `Network.requestWillBeSent` | HTTP request about to be sent |
| `Network.responseReceived` | HTTP response received |
| `Network.loadingFinished` | Response body fully loaded |

### OXI (AI Extensions)

OxiBrowser's proprietary domain for AI agent workflows.

| Method | Parameters | Description |
|--------|-----------|-------------|
| `OXI.getMarkdown` | — | Get page content as markdown |
| `OXI.getPageInfo` | — | Get structured page metadata |

**OXI.getMarkdown response:**

```json
{
    "markdown": "# Page Title\n\nPage content in markdown..."
}
```

**OXI.getPageInfo response:**

```json
{
    "url": "https://example.com",
    "title": "Example Domain",
    "statusCode": 200,
    "contentType": "text/html",
    "contentLength": 1256
}
```

### Page

| Method | Parameters | Description |
|--------|-----------|-------------|
| `Page.navigate` | `url, referrer?` | Navigate to URL |
| `Page.getFrameTree` | — | Get frame tree |
| `Page.getTitle` | — | Get page title |
| `Page.captureScreenshot` | `format?, quality?` | Capture screenshot |

**captureScreenshot** returns:

```json
{
    "data": "iVBORw0KGgoAAAANSUhEUgAA...",
    "metadata": {
        "pageWidth": 800,
        "pageHeight": 600
    }
}
```

`data` is base64-encoded PNG. Only `format: "png"` is supported.

### Runtime

| Method | Parameters | Description |
|--------|-----------|-------------|
| `Runtime.evaluate` | `expression, returnByValue?` | Evaluate JS expression |
| `Runtime.callFunctionOn` | `functionDeclaration, objectId?, arguments?` | Call JS function |
| `Runtime.enable` | — | Enable runtime events |
| `Runtime.disable` | — | Disable runtime events |

**Events:**

| Event | Description |
|-------|-------------|
| `Runtime.executionContextCreated` | New execution context available |
| `Runtime.consoleAPICalled` | `console.log` output |

### Target

| Method | Parameters | Description |
|--------|-----------|-------------|
| `Target.getTargets` | — | List available targets |
| `Target.attachToTarget` | `targetId` | Attach to target |
| `Target.detachFromTarget` | `sessionId` | Detach from target |
| `Target.createTarget` | `url` | Create new target (page) |

## Events

All events are broadcast to all connected WebSocket clients:

| Event | Domain | Description |
|-------|--------|-------------|
| `Page.frameNavigated` | Page | Navigation completed |
| `Page.domContentLoadedEventFired` | Page | DOM content loaded |
| `Page.loadEventFired` | Page | Page fully loaded |
| `Network.requestWillBeSent` | Network | HTTP request initiated |
| `Network.responseReceived` | Network | HTTP response received |
| `Network.loadingFinished` | Network | Response body loaded |
| `Runtime.executionContextCreated` | Runtime | JS context ready |
| `Runtime.consoleAPICalled` | Runtime | Console output |
| `Fetch.requestPaused` | Fetch | Request paused for interception |
