//! CDP server — TCP listener with HTTP and WebSocket endpoints.
//!
//! Provides:
//! - GET /json/version — browser metadata
//! - GET /json — list of debuggable targets
//! - WebSocket upgrade for CDP message dispatch

use crate::protocol::{JsonTarget, JsonVersion};
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use http_body_util::Full;
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

/// Type alias for HTTP response body.
type HttpBody = Full<Bytes>;

/// CDP server that listens for HTTP/WebSocket connections.
pub struct CdpServer {
    /// Address to bind to.
    addr: SocketAddr,
    /// Shutdown signal sender.
    shutdown_tx: broadcast::Sender<()>,
}

impl CdpServer {
    /// Create a new CDP server bound to the given address.
    pub fn new(addr: SocketAddr) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        Self { addr, shutdown_tx }
    }

    /// Start the CDP server.
    ///
    /// Returns the actual bound address (useful when port 0 is used).
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
                            info!(peer = %peer_addr, "new connection");

                            let server = self.clone();
                            tokio::spawn(async move {
                                if let Err(e) = server.handle_connection(stream, peer_addr).await {
                                    warn!(peer = %peer_addr, error = %e, "connection error");
                                }
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

        let io = TokioIo::new(stream);

        let service = service_fn(move |req: Request<hyper::body::Incoming>| {
            let ws_url = ws_url.clone();
            async move {
                self.handle_http_request(req, &ws_url).await
            }
        });

        http1::Builder::new()
            .serve_connection(io, service)
            .with_upgrades()
            .await?;

        Ok(())
    }

    /// Handle an HTTP request, possibly upgrading to WebSocket.
    async fn handle_http_request(
        &self,
        req: Request<hyper::body::Incoming>,
        ws_url: &str,
    ) -> anyhow::Result<Response<HttpBody>> {
        match req.uri().path() {
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
                    web_socket_debugger_url: format!("{}/ws", ws_url),
                }];
                let body = serde_json::to_string(&targets)?;
                Ok(Response::builder()
                    .header("Content-Type", "application/json")
                    .body(Full::new(Bytes::from(body)))?)
            }
            "/ws" => {
                // WebSocket upgrade handled separately via hyper upgrades
                // For now, return a simple response indicating WS endpoint
                Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from(
                        "WebSocket upgrade required. Use a WebSocket client.",
                    )))?)
            }
            _ => Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("Not Found")))?),
        }
    }
}
