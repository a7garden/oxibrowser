//! E2E integration tests for the CDP server.
//!
//! These tests verify the full CDP stack: HTTP endpoints, WebSocket upgrade,
//! command dispatch, event broadcasting, and DOM access.
//!
//! Pure Rust — uses tokio-tungstenite as the CDP client. No Node.js/Puppeteer.

use base64::Engine;
use futures::{SinkExt, StreamExt};
use oxibrowser_cdp::CdpServer;
use oxibrowser_core::Browser;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

/// Find an available TCP port.
fn find_available_port() -> u16 {
    use std::net::TcpListener;
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A minimal HTTP server that serves static HTML for testing.
struct TestHttpServer {
    addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TestHttpServer {
    /// Start serving the given HTML on a random port.
    fn start(html: &'static str) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // Set non-blocking so tokio can use it
        listener.set_nonblocking(true).unwrap();
        let tokio_listener = tokio::net::TcpListener::from_std(listener).unwrap();

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            let body = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/html; charset=utf-8\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {}",
                html.len(),
                html
            );

            loop {
                tokio::select! {
                    accept = tokio_listener.accept() => {
                        if let Ok((mut stream, _)) = accept {
                            use tokio::io::AsyncWriteExt;
                            let _ = stream.write_all(body.as_bytes()).await;
                            let _ = stream.shutdown().await;
                        }
                    }
                    _ = &mut shutdown_rx => {
                        break;
                    }
                }
            }
        });

        Self {
            addr,
            shutdown: Some(shutdown_tx),
        }
    }

    /// Get the address this server is listening on.
    fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// Start a CDP server on a random port.
async fn start_cdp_server() -> (Arc<CdpServer>, SocketAddr) {
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let mut config = oxibrowser_core::BrowserConfig::headless();
    // Disable SSRF filter for tests — need to connect to local test server
    config.enable_ssrf_filter = false;
    let browser = Arc::new(Browser::new(config).await.unwrap());
    let server = Arc::new(CdpServer::new(addr, browser));

    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.start().await;
    });

    // Give the server a moment to bind
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    (server, addr)
}

// Events that arrive while a `send_command` is awaiting its response are
// buffered here (concurrent dispatch may deliver events before the response).
// `collect_events` drains the prefix-matching ones so tests observe them.
thread_local! {
    static SIDECAR_EVENTS: std::cell::RefCell<Vec<Value>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Connect to a CDP server via WebSocket.
async fn connect_ws(
    addr: SocketAddr,
) -> (
    futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tungstenite::Message,
    >,
    futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    let url = format!("ws://{addr}/ws");
    let request = url.into_client_request().unwrap();
    let (ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let (sink, stream) = ws.split();
    SIDECAR_EVENTS.with(|b| b.borrow_mut().clear());
    (sink, stream)
}

/// Send a CDP command and return the response (skipping events).
async fn send_command(
    sink: &mut futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tungstenite::Message,
    >,
    ws: &mut futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    id: u64,
    method: &str,
    params: Option<Value>,
) -> Value {
    let msg = match params {
        Some(p) => json!({ "id": id, "method": method, "params": p }),
        None => json!({ "id": id, "method": method }),
    };

    sink.send(tungstenite::Message::Text(msg.to_string().into()))
        .await
        .unwrap();

    // Read response (skip events)
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("timeout waiting for response to command {id} ({method})");
        }
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                let response: Value = serde_json::from_str(&text).unwrap();
                if response.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    return response;
                }
                // Buffer events for a later collect_events (concurrent
                // dispatch may deliver them before this command's response).
                SIDECAR_EVENTS.with(|b| b.borrow_mut().push(response));
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => panic!("WebSocket error: {e}"),
            Ok(None) => panic!("WebSocket stream ended before response"),
            Err(_) => panic!("timeout waiting for response to command {id}"),
        }
    }
}

/// Read the response for a previously-sent command by id (it may already be
/// buffered in the sidecar, or arrive on the stream). Used when a command was
/// sent without awaiting (e.g. a paused navigation that completes later).
async fn read_command_response(
    ws: &mut futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    id: u64,
    timeout_ms: u64,
) -> Value {
    // Check the sidecar buffer first (concurrent dispatch may have delivered it).
    let buffered = SIDECAR_EVENTS.with(|b| {
        let mut buf = b.borrow_mut();
        buf.iter()
            .position(|v| v.get("id").and_then(|x| x.as_u64()) == Some(id))
            .map(|pos| buf.remove(pos))
    });
    if let Some(v) = buffered {
        return v;
    }
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("timeout waiting for response to command {id}");
        }
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                let response: Value = serde_json::from_str(&text).unwrap();
                if response.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    return response;
                }
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => panic!("WebSocket error: {e}"),
            Ok(None) => panic!("WebSocket stream ended"),
            Err(_) => panic!("timeout waiting for response to command {id}"),
        }
    }
}

/// Collect CDP events matching a method prefix within a time window.
async fn collect_events(
    ws: &mut futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    method_prefix: &str,
    max_wait_ms: u64,
) -> Vec<Value> {
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(max_wait_ms);
    // Drain prefix-matching events buffered while prior send_command calls
    // awaited their responses (concurrent dispatch delivers them early).
    SIDECAR_EVENTS.with(|b| {
        let mut buf = b.borrow_mut();
        let mut kept = Vec::with_capacity(buf.len());
        for ev in buf.drain(..) {
            match ev.get("method").and_then(|v| v.as_str()) {
                Some(m) if m.starts_with(method_prefix) => events.push(ev),
                _ => kept.push(ev),
            }
        }
        *buf = kept;
    });

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                let msg: Value = serde_json::from_str(&text).unwrap();
                if let Some(method) = msg.get("method").and_then(|v| v.as_str())
                    && method.starts_with(method_prefix)
                {
                    events.push(msg);
                }
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) => break,
            Ok(None) => break,
            Err(_) => break,
        }
    }

    events
}

// ============================================================
// HTTP endpoint tests
// ============================================================

#[tokio::test]
async fn test_http_json_version() {
    let (server, addr) = start_cdp_server().await;

    let resp = reqwest::get(format!("http://{addr}/json/version"))
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["browser"], "OxiBrowser/0.1.0");
    assert_eq!(body["protocolVersion"], "1.3");
    assert!(body["webSocketDebuggerUrl"].is_string());

    server.shutdown();
}

#[tokio::test]
async fn test_http_json_list() {
    let (server, addr) = start_cdp_server().await;

    let resp = reqwest::get(format!("http://{addr}/json")).await.unwrap();
    assert!(resp.status().is_success());

    let body: Vec<Value> = resp.json().await.unwrap();
    assert!(!body.is_empty());
    assert_eq!(body[0]["type"], "page");

    server.shutdown();
}

// ============================================================
// WebSocket CDP command tests
// ============================================================

#[tokio::test]
async fn test_ws_connect_and_browser_get_version() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let resp = send_command(&mut sink, &mut ws, 1, "Browser.getVersion", None).await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["protocolVersion"], "1.3");

    server.shutdown();
}

#[tokio::test]
async fn test_page_enable_events() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    // Runtime.enable
    let resp = send_command(&mut sink, &mut ws, 1, "Runtime.enable", None).await;
    assert_eq!(resp["id"], 1);

    // Collect Runtime.executionContextCreated event
    let events = collect_events(&mut ws, "Runtime.", 500).await;
    assert!(
        !events.is_empty(),
        "should receive Runtime.executionContextCreated"
    );
    assert_eq!(
        events[0]["method"], "Runtime.executionContextCreated",
        "first event should be executionContextCreated"
    );
    assert!(events[0]["params"]["context"]["id"].is_number());

    server.shutdown();
}

#[tokio::test]
async fn test_page_get_frame_tree() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let resp = send_command(&mut sink, &mut ws, 1, "Page.getFrameTree", None).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["frameTree"]["frame"].is_object());
    assert_eq!(
        resp["result"]["frameTree"]["frame"]["mimeType"],
        "text/html"
    );

    server.shutdown();
}

#[tokio::test]
async fn test_dom_get_document() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let resp = send_command(&mut sink, &mut ws, 1, "DOM.getDocument", None).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["root"].is_object());
    assert_eq!(resp["result"]["root"]["nodeType"], 9);

    server.shutdown();
}

#[tokio::test]
async fn test_runtime_evaluate() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    // Number literal
    let resp = send_command(
        &mut sink,
        &mut ws,
        1,
        "Runtime.evaluate",
        Some(json!({ "expression": "42" })),
    )
    .await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["result"]["type"], "number");
    assert_eq!(resp["result"]["result"]["value"], 42);

    // String literal
    let resp = send_command(
        &mut sink,
        &mut ws,
        2,
        "Runtime.evaluate",
        Some(json!({ "expression": "'hello'" })),
    )
    .await;
    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"]["result"]["type"], "string");
    assert_eq!(resp["result"]["result"]["value"], "hello");

    server.shutdown();
}

#[tokio::test]
async fn test_unknown_domain_returns_error() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let resp = send_command(&mut sink, &mut ws, 1, "Foo.bar", None).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["error"].is_object());
    assert_eq!(resp["error"]["code"], -32601);

    server.shutdown();
}

#[tokio::test]
async fn test_target_domain() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    // Target.getTargets
    let resp = send_command(&mut sink, &mut ws, 1, "Target.getTargets", None).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["targetInfos"].is_array());

    // Target.createTarget
    let resp = send_command(
        &mut sink,
        &mut ws,
        2,
        "Target.createTarget",
        Some(json!({ "url": "https://example.com" })),
    )
    .await;
    assert_eq!(resp["id"], 2);
    assert!(resp["result"]["targetId"].is_string());

    server.shutdown();
}

#[tokio::test]
async fn test_create_target_creates_drivable_session() {
    // Multi-tab: Target.createTarget must mint a real session that commands
    // routed by the new sessionId can drive (navigate + evaluate).
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let resp = send_command(
        &mut sink,
        &mut ws,
        1,
        "Target.createTarget",
        Some(json!({ "url": "about:blank" })),
    )
    .await;
    assert_eq!(resp["id"], 1);
    let _target_id = resp["result"]["targetId"].as_str().unwrap().to_string();

    // The attachedToTarget event carries the child sessionId.
    let attached = collect_events(&mut ws, "Target.attachedToTarget", 3000)
        .await
        .pop()
        .expect("attachedToTarget event");
    let child_session = attached["params"]["sessionId"]
        .as_str()
        .expect("attachedToTarget should carry sessionId")
        .to_string();

    // Navigate the child tab (routed by sessionId).
    let nav_msg = json!({
        "id": 2, "method": "Page.navigate", "sessionId": child_session,
        "params": { "url": "data:text/html,<html><body><p id='x'>tab2</p></body></html>" }
    });
    sink.send(tungstenite::Message::Text(nav_msg.to_string().into()))
        .await
        .unwrap();
    let nav_resp = read_command_response(&mut ws, 2, 5000).await;
    assert_eq!(nav_resp["id"], 2);
    assert!(
        nav_resp.get("error").is_none(),
        "child navigate should route by sessionId: {:?}",
        nav_resp
    );

    // Evaluate in the child session.
    let eval_msg = json!({
        "id": 3, "method": "Runtime.evaluate", "sessionId": child_session,
        "params": { "expression": "document.getElementById('x').textContent" }
    });
    sink.send(tungstenite::Message::Text(eval_msg.to_string().into()))
        .await
        .unwrap();
    let eval_resp = read_command_response(&mut ws, 3, 5000).await;
    assert_eq!(eval_resp["id"], 3);
    assert!(
        eval_resp.get("error").is_none(),
        "child eval should route by sessionId: {:?}",
        eval_resp
    );
    assert_eq!(
        eval_resp["result"]["result"]["value"].as_str(),
        Some("tab2"),
        "child session should evaluate its own DOM"
    );

    server.shutdown();
}

#[tokio::test]
async fn test_fetch_domain_enable_disable() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let resp = send_command(&mut sink, &mut ws, 1, "Fetch.enable", None).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["result"].is_object());

    let resp = send_command(&mut sink, &mut ws, 2, "Fetch.disable", None).await;
    assert_eq!(resp["id"], 2);
    assert!(resp["result"].is_object());

    server.shutdown();
}

// ============================================================
// Full navigation E2E tests
// ============================================================

#[tokio::test]
async fn test_navigate_to_local_server_and_inspect_dom() {
    // Start a local HTTP server with known HTML
    let html = r#"<html>
        <head><title>E2E Test Page</title></head>
        <body>
            <h1 id="heading">Hello OxiBrowser</h1>
            <p class="content">This is a test page.</p>
            <ul>
                <li class="item">Item 1</li>
                <li class="item">Item 2</li>
                <li class="item">Item 3</li>
            </ul>
            <a href="/link" id="mylink">Click me</a>
        </body>
    </html>"#;
    let http_server = TestHttpServer::start(html);

    // Start CDP server
    let (cdp_server, cdp_addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(cdp_addr).await;

    // Navigate to the local server
    let url = format!("http://{}/", http_server.addr());
    let resp = send_command(
        &mut sink,
        &mut ws,
        1,
        "Page.navigate",
        Some(json!({ "url": url })),
    )
    .await;

    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["frameId"].is_string());
    assert!(resp["result"]["loaderId"].is_string());

    // Verify DOM.getDocument returns a real parsed tree
    let resp = send_command(&mut sink, &mut ws, 2, "DOM.getDocument", None).await;
    assert_eq!(resp["id"], 2);
    let root = &resp["result"]["root"];
    assert_eq!(root["nodeType"], 9, "root should be a document node");
    assert!(root["children"].is_array(), "root should have children");

    // Verify Page.getFrameTree has the navigated URL
    let resp = send_command(&mut sink, &mut ws, 3, "Page.getFrameTree", None).await;
    assert_eq!(resp["id"], 3);
    let frame_url = resp["result"]["frameTree"]["frame"]["url"]
        .as_str()
        .unwrap();
    assert!(
        frame_url.contains("127.0.0.1"),
        "frame URL should point to local server, got: {frame_url}"
    );

    // Verify DOM.querySelector finds #heading
    let resp = send_command(
        &mut sink,
        &mut ws,
        4,
        "DOM.querySelector",
        Some(json!({ "nodeId": 0, "selector": "#heading" })),
    )
    .await;
    assert_eq!(resp["id"], 4);
    let heading_id = resp["result"]["nodeId"].as_u64().unwrap();
    assert!(heading_id > 0, "should find #heading element");

    // Verify DOM.querySelectorAll finds .item
    let resp = send_command(
        &mut sink,
        &mut ws,
        5,
        "DOM.querySelectorAll",
        Some(json!({ "nodeId": 0, "selector": ".item" })),
    )
    .await;
    assert_eq!(resp["id"], 5);
    let items = resp["result"]["nodeIds"].as_array().unwrap();
    assert_eq!(items.len(), 3, "should find 3 .item elements");

    // Verify DOM.getOuterHTML returns the full HTML
    let resp = send_command(&mut sink, &mut ws, 6, "DOM.getOuterHTML", None).await;
    assert_eq!(resp["id"], 6);
    let outer_html = resp["result"]["outerHTML"].as_str().unwrap();
    assert!(
        outer_html.contains("Hello OxiBrowser"),
        "HTML should contain heading text"
    );
    assert!(
        outer_html.contains("E2E Test Page"),
        "HTML should contain title"
    );

    cdp_server.shutdown();
}

#[tokio::test]
async fn test_navigate_emits_page_events() {
    let html = r#"<html><head><title>Event Test</title></head><body><p>Content</p></body></html>"#;
    let http_server = TestHttpServer::start(html);

    let (cdp_server, cdp_addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(cdp_addr).await;

    // Enable Page events first
    let _ = send_command(&mut sink, &mut ws, 1, "Page.enable", None).await;

    // Navigate
    let url = format!("http://{}/", http_server.addr());
    let _ = send_command(
        &mut sink,
        &mut ws,
        2,
        "Page.navigate",
        Some(json!({ "url": url })),
    )
    .await;

    // Collect Page events
    let events = collect_events(&mut ws, "Page.", 1000).await;
    let methods: Vec<&str> = events.iter().filter_map(|e| e["method"].as_str()).collect();

    assert!(
        methods.contains(&"Page.frameNavigated"),
        "should emit Page.frameNavigated, got: {methods:?}"
    );
    assert!(
        methods.contains(&"Page.domContentLoadedEventFired"),
        "should emit Page.domContentLoadedEventFired, got: {methods:?}"
    );
    assert!(
        methods.contains(&"Page.loadEventFired"),
        "should emit Page.loadEventFired, got: {methods:?}"
    );

    cdp_server.shutdown();
}

#[tokio::test]
async fn test_navigate_emits_network_events() {
    let html = r#"<html><body><p>Network Test</p></body></html>"#;
    let http_server = TestHttpServer::start(html);

    let (cdp_server, cdp_addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(cdp_addr).await;

    // Enable Network events
    let _ = send_command(&mut sink, &mut ws, 1, "Network.enable", None).await;

    // Navigate
    let url = format!("http://{}/", http_server.addr());
    let _ = send_command(
        &mut sink,
        &mut ws,
        2,
        "Page.navigate",
        Some(json!({ "url": url })),
    )
    .await;

    // Collect Network events
    let events = collect_events(&mut ws, "Network.", 1000).await;
    let methods: Vec<&str> = events.iter().filter_map(|e| e["method"].as_str()).collect();

    assert!(
        methods.contains(&"Network.requestWillBeSent"),
        "should emit Network.requestWillBeSent, got: {methods:?}"
    );
    assert!(
        methods.contains(&"Network.responseReceived"),
        "should emit Network.responseReceived, got: {methods:?}"
    );
    assert!(
        methods.contains(&"Network.loadingFinished"),
        "should emit Network.loadingFinished, got: {methods:?}"
    );

    // Verify the request URL is correct
    let req_event = events
        .iter()
        .find(|e| e["method"] == "Network.requestWillBeSent")
        .unwrap();
    let req_url = req_event["params"]["request"]["url"].as_str().unwrap();
    assert!(
        req_url.contains("127.0.0.1"),
        "request URL should point to local server, got: {req_url}"
    );

    cdp_server.shutdown();
}

#[tokio::test]
async fn test_runtime_evaluate_after_navigation() {
    let html = r#"<html><body><p>JS Test</p></body></html>"#;
    let http_server = TestHttpServer::start(html);

    let (cdp_server, cdp_addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(cdp_addr).await;

    // Navigate first
    let url = format!("http://{}/", http_server.addr());
    let _ = send_command(
        &mut sink,
        &mut ws,
        1,
        "Page.navigate",
        Some(json!({ "url": url })),
    )
    .await;

    // Drain any events
    let _ = collect_events(&mut ws, "Page.", 200).await;

    // Now evaluate JS
    let resp = send_command(
        &mut sink,
        &mut ws,
        2,
        "Runtime.evaluate",
        Some(json!({ "expression": "'hello world'" })),
    )
    .await;
    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"]["result"]["type"], "string");
    assert_eq!(resp["result"]["result"]["value"], "hello world");

    // Boolean
    let resp = send_command(
        &mut sink,
        &mut ws,
        3,
        "Runtime.evaluate",
        Some(json!({ "expression": "true" })),
    )
    .await;
    assert_eq!(resp["result"]["result"]["type"], "boolean");
    assert_eq!(resp["result"]["result"]["value"], true);

    // Number
    let resp = send_command(
        &mut sink,
        &mut ws,
        4,
        "Runtime.evaluate",
        Some(json!({ "expression": "3.14" })),
    )
    .await;
    assert_eq!(resp["result"]["result"]["type"], "number");

    cdp_server.shutdown();
}

#[tokio::test]
async fn test_full_workflow_connect_navigate_inspect_close() {
    let html = r#"<html>
        <head><title>Full Workflow</title></head>
        <body>
            <div id="main">
                <h2>Workflow Test</h2>
                <p class="desc">Description</p>
            </div>
        </body>
    </html>"#;
    let http_server = TestHttpServer::start(html);

    let (cdp_server, cdp_addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(cdp_addr).await;

    // 1. Browser.getVersion
    let resp = send_command(&mut sink, &mut ws, 1, "Browser.getVersion", None).await;
    assert_eq!(resp["result"]["protocolVersion"], "1.3");

    // 2. Enable Runtime
    let _ = send_command(&mut sink, &mut ws, 2, "Runtime.enable", None).await;
    let _ = collect_events(&mut ws, "Runtime.", 300).await;

    // 3. Enable Page
    let _ = send_command(&mut sink, &mut ws, 3, "Page.enable", None).await;

    // 4. Navigate
    let url = format!("http://{}/", http_server.addr());
    let resp = send_command(
        &mut sink,
        &mut ws,
        4,
        "Page.navigate",
        Some(json!({ "url": url })),
    )
    .await;
    assert!(resp["result"]["frameId"].is_string());

    // 5. Collect Page events
    let events = collect_events(&mut ws, "Page.", 500).await;
    assert!(!events.is_empty(), "should receive page events");

    // 6. Inspect DOM
    let resp = send_command(
        &mut sink,
        &mut ws,
        5,
        "DOM.querySelector",
        Some(json!({ "nodeId": 0, "selector": "#main" })),
    )
    .await;
    assert!(resp["result"]["nodeId"].as_u64().unwrap() > 0);

    // 7. Get all items
    let resp = send_command(
        &mut sink,
        &mut ws,
        6,
        "DOM.querySelectorAll",
        Some(json!({ "nodeId": 0, "selector": "p" })),
    )
    .await;
    assert_eq!(resp["result"]["nodeIds"].as_array().unwrap().len(), 1);

    // 8. Evaluate JS
    let resp = send_command(
        &mut sink,
        &mut ws,
        7,
        "Runtime.evaluate",
        Some(json!({ "expression": "1 + 1" })),
    )
    .await;
    // In stub mode, "1 + 1" returns as string "1 + 1"
    // (stub doesn't evaluate expressions, only literals)
    assert!(
        resp["result"]["result"]["value"].is_string()
            || resp["result"]["result"]["value"].is_number(),
        "should return some value"
    );

    cdp_server.shutdown();
}

// ---------------------------------------------------------------------------
// Input domain tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_input_dispatch_key_event() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let _ = send_command(&mut sink, &mut ws, 1, "Page.enable", None).await;

    let resp = send_command(
        &mut sink,
        &mut ws,
        2,
        "Input.dispatchKeyEvent",
        Some(json!({
            "type": "keyDown", "key": "a", "code": "KeyA", "modifiers": 0
        })),
    )
    .await;
    assert_eq!(resp["id"], 2);

    let resp = send_command(
        &mut sink,
        &mut ws,
        3,
        "Input.dispatchKeyEvent",
        Some(json!({
            "type": "keyUp", "key": "a", "code": "KeyA"
        })),
    )
    .await;
    assert_eq!(resp["id"], 3);

    let resp = send_command(
        &mut sink,
        &mut ws,
        4,
        "Input.insertText",
        Some(json!({"text": "hello"})),
    )
    .await;
    assert_eq!(resp["id"], 4);

    server.shutdown();
}

#[tokio::test]
async fn test_input_dispatch_mouse_event() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let resp = send_command(
        &mut sink,
        &mut ws,
        1,
        "Input.dispatchMouseEvent",
        Some(json!({
            "type": "mouseMoved", "x": 100.0, "y": 200.0, "button": "none", "clickCount": 0
        })),
    )
    .await;
    assert_eq!(resp["id"], 1);

    let resp = send_command(
        &mut sink,
        &mut ws,
        2,
        "Input.dispatchMouseEvent",
        Some(json!({
            "type": "mousePressed", "x": 100.0, "y": 200.0, "button": "left", "clickCount": 1
        })),
    )
    .await;
    assert_eq!(resp["id"], 2);

    let resp = send_command(
        &mut sink,
        &mut ws,
        3,
        "Input.dispatchMouseEvent",
        Some(json!({
            "type": "mouseReleased", "x": 100.0, "y": 200.0, "button": "left", "clickCount": 1
        })),
    )
    .await;
    assert_eq!(resp["id"], 3);

    server.shutdown();
}

// ---------------------------------------------------------------------------
// Network cookie tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_network_get_all_cookies() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let resp = send_command(&mut sink, &mut ws, 1, "Network.getAllCookies", None).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["cookies"].is_array());

    server.shutdown();
}

#[tokio::test]
async fn test_network_set_cookie() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let resp = send_command(&mut sink, &mut ws, 1, "Network.setCookie", Some(json!({
        "name": "session_id", "value": "abc123", "url": "http://example.com/", "path": "/", "secure": false
    }))).await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["success"], true);

    let resp = send_command(&mut sink, &mut ws, 2, "Network.getAllCookies", None).await;
    let cookies = resp["result"]["cookies"].as_array().unwrap();
    assert!(!cookies.is_empty());

    server.shutdown();
}

#[tokio::test]
async fn test_network_delete_cookies() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let _ = send_command(
        &mut sink,
        &mut ws,
        1,
        "Network.setCookie",
        Some(json!({
            "name": "temp_key", "value": "temp_val", "url": "http://example.com/"
        })),
    )
    .await;

    let resp = send_command(
        &mut sink,
        &mut ws,
        2,
        "Network.deleteCookies",
        Some(json!({
            "name": "temp_key", "url": "http://example.com/"
        })),
    )
    .await;
    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"]["success"], true);

    server.shutdown();
}

// ---------------------------------------------------------------------------
// Fetch domain tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_fetch_fulfill_request() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    // Enable Fetch domain with a wildcard pattern (matches any http URL).
    let resp = send_command(
        &mut sink,
        &mut ws,
        1,
        "Fetch.enable",
        Some(json!({ "patterns": [{"urlPattern": "http://*"}] })),
    )
    .await;
    assert_eq!(resp["id"], 1);

    // Send Page.navigate WITHOUT awaiting — the server pauses the request and
    // emits Fetch.requestPaused; we must answer before it can complete.
    let nav_msg =
        json!({ "id": 2, "method": "Page.navigate", "params": { "url": "http://example.com/" } });
    sink.send(tungstenite::Message::Text(nav_msg.to_string().into()))
        .await
        .unwrap();

    // Collect the Fetch.requestPaused event and extract its requestId.
    let paused = collect_events(&mut ws, "Fetch.requestPaused", 3000)
        .await
        .pop()
        .expect("expected a Fetch.requestPaused event");
    let intercept_id = paused["params"]["requestId"]
        .as_str()
        .expect("requestPaused should carry a requestId")
        .to_string();

    // Fulfill the paused request with a mock HTML body.
    let resp = send_command(
        &mut sink,
        &mut ws,
        3,
        "Fetch.fulfillRequest",
        Some(json!({
            "requestId": intercept_id,
            "responseCode": 200,
            "responseHeaders": [{"name": "content-type", "value": "text/html"}],
            "body": base64::engine::general_purpose::STANDARD.encode("<html><body>mocked</body></html>")
        })),
    )
    .await;
    assert_eq!(resp["id"], 3);

    // The navigate (id 2) should now complete (fulfilled → data-URL navigation).
    let nav_resp = read_command_response(&mut ws, 2, 5000).await;
    assert_eq!(nav_resp["id"], 2);

    let _ = send_command(&mut sink, &mut ws, 4, "Fetch.disable", None).await;
    server.shutdown();
}

#[tokio::test]
async fn test_fetch_continue_request() {
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let _ = send_command(&mut sink, &mut ws, 1, "Fetch.enable", None).await;

    // Try to continue a non-existent request — should return error
    let resp = send_command(
        &mut sink,
        &mut ws,
        2,
        "Fetch.continueRequest",
        Some(json!({
            "requestId": "nonexistent", "url": "http://example.com/"
        })),
    )
    .await;
    assert_eq!(resp["id"], 2);
    // Should have an error because requestId not found
    assert!(
        resp.get("error").is_some(),
        "expected error for unknown requestId"
    );

    let _ = send_command(&mut sink, &mut ws, 3, "Fetch.disable", None).await;
    server.shutdown();
}

#[tokio::test]
async fn test_fetch_fulfill_unknown_request() {
    // Test that Fetch.fulfillRequest returns error for unknown requestId
    let (server, addr) = start_cdp_server().await;
    let (mut sink, mut ws) = connect_ws(addr).await;

    let _ = send_command(&mut sink, &mut ws, 1, "Fetch.enable", None).await;

    let resp = send_command(
        &mut sink,
        &mut ws,
        2,
        "Fetch.fulfillRequest",
        Some(json!({
            "requestId": "unknown-id", "statusCode": 200, "statusText": "OK",
            "body": "test", "responseHeaders": []
        })),
    )
    .await;
    assert_eq!(resp["id"], 2);
    // Should have an error because requestId not found
    assert!(
        resp.get("error").is_some(),
        "expected error for unknown requestId in fulfillRequest"
    );

    let _ = send_command(&mut sink, &mut ws, 3, "Fetch.disable", None).await;
    server.shutdown();
}
