//! E2E integration tests for the CDP server.
//!
//! These tests verify the full CDP stack: HTTP endpoints, WebSocket upgrade,
//! command dispatch, event broadcasting, and DOM access.
//!
//! Pure Rust — uses tokio-tungstenite as the CDP client. No Node.js/Puppeteer.

use futures::{SinkExt, StreamExt};
use oxibrowser_cdp::CdpServer;
use oxibrowser_core::Browser;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// Find an available TCP port.
fn find_available_port() -> u16 {
    use std::net::TcpListener;
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Helper: connect to the CDP server via WebSocket.
async fn connect_ws(addr: SocketAddr) -> (
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
    ws.split()
}

/// Helper: send a CDP command and return the response.
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

    // Read response
    while let Some(result) = ws.next().await {
        match result {
            Ok(tungstenite::Message::Text(text)) => {
                let response: Value = serde_json::from_str(&text).unwrap();
                if response.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    return response;
                }
                // Skip events (no "id" field) — they're expected
            }
            Ok(_) => continue,
            Err(e) => panic!("WebSocket error: {e}"),
        }
    }

    panic!("no response received for command {id}");
}

/// Helper: collect CDP events matching a method prefix.
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

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(tungstenite::Message::Text(text)))) => {
                let msg: Value = serde_json::from_str(&text).unwrap();
                if let Some(method) = msg.get("method").and_then(|v| v.as_str()) {
                    if method.starts_with(method_prefix) {
                        events.push(msg);
                    }
                }
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) => break,
            Ok(None) => break,
            Err(_) => break, // timeout
        }
    }

    events
}

// ============================================================
// Tests
// ============================================================

#[tokio::test]
async fn test_http_json_version() {
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = Arc::new(Browser::new(config).await.unwrap());
    let server = Arc::new(CdpServer::new(addr, browser));

    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.start().await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // GET /json/version
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
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = Arc::new(Browser::new(config).await.unwrap());
    let server = Arc::new(CdpServer::new(addr, browser));

    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.start().await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // GET /json
    let resp = reqwest::get(format!("http://{addr}/json"))
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let body: Vec<Value> = resp.json().await.unwrap();
    assert!(!body.is_empty());
    assert_eq!(body[0]["type"], "page");

    server.shutdown();
}

#[tokio::test]
async fn test_ws_connect_and_browser_get_version() {
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = Arc::new(Browser::new(config).await.unwrap());
    let server = Arc::new(CdpServer::new(addr, browser));

    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.start().await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let (mut sink, mut ws) = connect_ws(addr).await;

    // Browser.getVersion
    let resp = send_command(&mut sink, &mut ws, 1, "Browser.getVersion", None).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["protocolVersion"].is_string());
    assert_eq!(resp["result"]["protocolVersion"], "1.3");

    server.shutdown();
}

#[tokio::test]
async fn test_page_enable_events() {
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = Arc::new(Browser::new(config).await.unwrap());
    let server = Arc::new(CdpServer::new(addr, browser));

    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.start().await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let (mut sink, mut ws) = connect_ws(addr).await;

    // Runtime.enable
    let resp = send_command(&mut sink, &mut ws, 1, "Runtime.enable", None).await;
    assert_eq!(resp["id"], 1);

    // Collect Runtime.executionContextCreated event
    let events = collect_events(&mut ws, "Runtime.", 500).await;
    assert!(!events.is_empty(), "should receive Runtime.executionContextCreated");
    assert_eq!(
        events[0]["method"], "Runtime.executionContextCreated",
        "first event should be executionContextCreated"
    );

    server.shutdown();
}

#[tokio::test]
async fn test_page_get_frame_tree() {
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = Arc::new(Browser::new(config).await.unwrap());
    let server = Arc::new(CdpServer::new(addr, browser));

    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.start().await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let (mut sink, mut ws) = connect_ws(addr).await;

    // Page.getFrameTree (no navigation yet — should return about:blank stub)
    let resp = send_command(&mut sink, &mut ws, 1, "Page.getFrameTree", None).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["frameTree"]["frame"].is_object());
    assert_eq!(resp["result"]["frameTree"]["frame"]["mimeType"], "text/html");

    server.shutdown();
}

#[tokio::test]
async fn test_dom_get_document() {
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = Arc::new(Browser::new(config).await.unwrap());
    let server = Arc::new(CdpServer::new(addr, browser));

    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.start().await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let (mut sink, mut ws) = connect_ws(addr).await;

    // DOM.getDocument (no page loaded — should return empty document)
    let resp = send_command(&mut sink, &mut ws, 1, "DOM.getDocument", None).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["root"].is_object());
    // Root is a document node (nodeType 9)
    assert_eq!(resp["result"]["root"]["nodeType"], 9);

    server.shutdown();
}

#[tokio::test]
async fn test_runtime_evaluate() {
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = Arc::new(Browser::new(config).await.unwrap());
    let server = Arc::new(CdpServer::new(addr, browser));

    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.start().await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let (mut sink, mut ws) = connect_ws(addr).await;

    // Runtime.evaluate — number literal
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

    // Runtime.evaluate — string literal
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
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = Arc::new(Browser::new(config).await.unwrap());
    let server = Arc::new(CdpServer::new(addr, browser));

    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.start().await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let (mut sink, mut ws) = connect_ws(addr).await;

    // Unknown domain
    let resp = send_command(&mut sink, &mut ws, 1, "Foo.bar", None).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["error"].is_object());
    assert_eq!(resp["error"]["code"], -32601);

    server.shutdown();
}

#[tokio::test]
async fn test_target_domain() {
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = Arc::new(Browser::new(config).await.unwrap());
    let server = Arc::new(CdpServer::new(addr, browser));

    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.start().await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

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
async fn test_fetch_domain_enable_disable() {
    let port = find_available_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = Arc::new(Browser::new(config).await.unwrap());
    let server = Arc::new(CdpServer::new(addr, browser));

    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.start().await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let (mut sink, mut ws) = connect_ws(addr).await;

    // Fetch.enable
    let resp = send_command(&mut sink, &mut ws, 1, "Fetch.enable", None).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["result"].is_object());

    // Fetch.disable
    let resp = send_command(&mut sink, &mut ws, 2, "Fetch.disable", None).await;
    assert_eq!(resp["id"], 2);
    assert!(resp["result"].is_object());

    server.shutdown();
}
