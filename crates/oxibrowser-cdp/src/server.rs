//! CDP server — TCP listener with HTTP and WebSocket endpoints.
//!
//! Provides:
//! - GET /json/version — browser metadata
//! - GET /json — list of debuggable targets
//! - WebSocket upgrade for CDP message dispatch
//!
//! The server holds a shared reference to the `Browser` instance and passes
//! it to each `CdpSession` created on WebSocket upgrade.

use crate::protocol::{JsonTarget, JsonVersion};
use crate::session::CdpSession;
use base64::Engine;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use oxibrowser_core::Browser;
use sha1::{Digest, Sha1};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::WebSocketStream;
use tracing::{error, info, warn};

/// Type alias for HTTP response body.
type HttpBody = Full<Bytes>;

/// WebSocket magic GUID for accept-key derivation (RFC 6455).
const WS_MAGIC_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Maximum number of concurrent CDP connections.
const MAX_CONCURRENT_CONNECTIONS: usize = 16;

/// Maximum allowed CDP message size (1 MB).
pub(crate) const MAX_CDP_MESSAGE_SIZE: usize = 1024 * 1024;

/// Derive the `Sec-WebSocket-Accept` value from the client's key.
fn derive_accept_key(client_key: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(client_key);
    hasher.update(WS_MAGIC_GUID);
    let hash = hasher.finalize();
    base64::engine::general_purpose::STANDARD.encode(hash)
}

/// CDP server that listens for HTTP/WebSocket connections.
pub struct CdpServer {
    /// Address to bind to.
    addr: SocketAddr,
    /// Shared browser instance.
    browser: Arc<Browser>,
    /// Shutdown signal sender.
    shutdown_tx: broadcast::Sender<()>,
    /// Active connection count.
    connection_count: Arc<AtomicUsize>,
}

impl CdpServer {
    /// Create a new CDP server bound to the given address with a shared Browser.
    pub fn new(addr: SocketAddr, browser: Arc<Browser>) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        Self {
            addr,
            browser,
            shutdown_tx,
            connection_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Start the CDP server.
    ///
    /// Returns the actual bound address (useful when port 0 is used).
    #[tracing::instrument(skip(self), fields(addr = %self.addr), err)]
    pub async fn start(self: &Arc<Self>) -> anyhow::Result<SocketAddr> {
        let listener = TcpListener::bind(self.addr).await?;
        let actual_addr = listener.local_addr()?;

        info!(addr = %actual_addr, "CDP server listening");

        let mut shutdown_rx = self.shutdown_tx.subscribe();

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, peer_addr)) => {
                            let active = self.connection_count.load(Ordering::Relaxed);
                            if active >= MAX_CONCURRENT_CONNECTIONS {
                                warn!(
                                    peer = %peer_addr,
                                    active,
                                    max = MAX_CONCURRENT_CONNECTIONS,
                                    "rejecting connection: too many concurrent connections"
                                );
                                continue;
                            }

                            info!(peer = %peer_addr, "new connection");

                            let server = self.clone();
                            let connection_count = self.connection_count.clone();
                            connection_count.fetch_add(1, Ordering::Relaxed);
                            tokio::spawn(async move {
                                if let Err(e) = server.handle_connection(stream, peer_addr).await {
                                    warn!(peer = %peer_addr, error = %e, "connection error");
                                }
                                connection_count.fetch_sub(1, Ordering::Relaxed);
                            });
                        }
                        Err(e) => {
                            error!(error = %e, "accept failed");
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("CDP server shutting down");
                    break;
                }
            }
        }

        Ok(actual_addr)
    }

    /// Signal the server to shut down.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
    }

    /// Handle a single TCP connection (HTTP upgrade or WebSocket).
    async fn handle_connection(
        &self,
        stream: tokio::net::TcpStream,
        _peer_addr: SocketAddr,
    ) -> anyhow::Result<()> {
        let ws_url = format!("ws://{}/ws", self.addr);
        let browser = self.browser.clone();

        let io = TokioIo::new(stream);

        let service = service_fn(move |req: Request<hyper::body::Incoming>| {
            let ws_url = ws_url.clone();
            let browser = browser.clone();
            async move { Self::handle_http_request(req, &ws_url, browser).await }
        });

        http1::Builder::new()
            .serve_connection(io, service)
            .with_upgrades()
            .await?;

        Ok(())
    }

    /// Handle an HTTP request, possibly upgrading to WebSocket.
    async fn handle_http_request(
        req: Request<hyper::body::Incoming>,
        ws_url: &str,
        browser: Arc<Browser>,
    ) -> anyhow::Result<Response<HttpBody>> {
        match req.uri().path() {
            "/health" => {
                let body = serde_json::json!({
                    "status": "ok",
                    "version": env!("CARGO_PKG_VERSION"),
                });
                Ok(Response::builder()
                    .header("Content-Type", "application/json")
                    .body(Full::new(Bytes::from(body.to_string())))?)
            }
            "/json/version" => {
                let version = JsonVersion::new(ws_url.to_string());
                let body = serde_json::to_string(&version)?;
                Ok(Response::builder()
                    .header("Content-Type", "application/json")
                    .body(Full::new(Bytes::from(body)))?)
            }
            "/json" | "/json/list" => {
                let targets = vec![JsonTarget {
                    id: "default".to_string(),
                    title: "OxiBrowser".to_string(),
                    target_type: "page".to_string(),
                    url: "about:blank".to_string(),
                    web_socket_debugger_url: ws_url.to_string(),
                }];
                let body = serde_json::to_string(&targets)?;
                Ok(Response::builder()
                    .header("Content-Type", "application/json")
                    .body(Full::new(Bytes::from(body)))?)
            }
            "/ws" => {
                // Check for WebSocket upgrade
                let is_ws_upgrade = req
                    .headers()
                    .get("upgrade")
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.eq_ignore_ascii_case("websocket"))
                    .unwrap_or(false);

                if !is_ws_upgrade {
                    return Ok(Response::builder().status(StatusCode::BAD_REQUEST).body(
                        Full::new(Bytes::from(
                            "WebSocket upgrade required. Use a WebSocket client.",
                        )),
                    )?);
                }

                // Extract the client's Sec-WebSocket-Key
                let client_key = req
                    .headers()
                    .get("sec-websocket-key")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default();

                let accept_key = derive_accept_key(client_key.as_bytes());

                // Spawn a task to handle the upgraded WebSocket connection.
                // After we return 101, hyper will hand over the raw IO.
                tokio::spawn(async move {
                    match hyper::upgrade::on(req).await {
                        Ok(upgraded) => {
                            // Wrap hyper's Upgraded IO with TokioIo to get
                            // tokio AsyncRead/AsyncWrite traits.
                            let io = TokioIo::new(upgraded);
                            let ws = WebSocketStream::from_raw_socket(io, Role::Server, None).await;

                            match CdpSession::new(ws, browser).await {
                                Ok(session) => {
                                    if let Err(e) = session.run().await {
                                        warn!(error = %e, "CDP session error");
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "failed to create CDP session");
                                }
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "WebSocket upgrade failed");
                        }
                    }
                });

                // Return 101 Switching Protocols with the accept key
                Ok(Response::builder()
                    .status(StatusCode::SWITCHING_PROTOCOLS)
                    .header("Upgrade", "websocket")
                    .header("Connection", "Upgrade")
                    .header("Sec-WebSocket-Accept", accept_key)
                    .body(Full::new(Bytes::new()))?)
            }
            _ => Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("Not Found")))?),
        }
    }
}
