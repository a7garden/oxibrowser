//! CDP session — a single WebSocket connection for CDP communication.
//!
//! Manages a WebSocket connection, dispatches incoming CDP commands to
//! domain handlers, and sends responses back to the client.

use crate::domains;
use crate::protocol::{CdpRequest, CdpResponse, CdpEvent};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite;
use tracing::{debug, error, info, warn};

/// A single CDP session over a WebSocket connection.
pub struct CdpSession {
    /// The WebSocket sink.
    sink: futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tungstenite::Message,
    >,
    /// The WebSocket stream.
    ws: futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    /// Session ID for this connection.
    session_id: String,
    /// Target ID this session is attached to.
    target_id: Option<String>,
}

impl CdpSession {
    /// Create a new CDP session wrapping a WebSocket stream.
    pub fn new(
        ws_stream: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Self {
        let (sink, ws) = ws_stream.split();
        let session_id = format!("session-{}", uuid::Uuid::new_v4());

        info!(session_id = %session_id, "CDP session created");

        Self {
            ws,
            sink,
            session_id,
            target_id: None,
        }
    }

    /// Run the message dispatch loop.
    ///
    /// Reads messages from the WebSocket, dispatches CDP commands,
    /// and sends responses back.
    pub async fn run(mut self) -> anyhow::Result<()> {
        info!(session_id = %self.session_id, "CDP session started");

        while let Some(msg) = self.ws.next().await {
            match msg {
                Ok(tungstenite::Message::Text(text)) => {
                    debug!(text = %text, "received CDP message");
                    self.handle_text_message(&text).await?;
                }
                Ok(tungstenite::Message::Close(_)) => {
                    info!(session_id = %self.session_id, "WebSocket closed by client");
                    break;
                }
                Ok(tungstenite::Message::Ping(data)) => {
                    self.sink
                        .send(tungstenite::Message::Pong(data))
                        .await?;
                }
                Ok(_) => {
                    // Binary, Pong, Frame — ignore
                }
                Err(e) => {
                    error!(error = %e, "WebSocket read error");
                    break;
                }
            }
        }

        info!(session_id = %self.session_id, "CDP session ended");
        Ok(())
    }

    /// Handle a single text message.
    async fn handle_text_message(&mut self, text: &str) -> anyhow::Result<()> {
        // Parse the CDP request
        let request: CdpRequest = match serde_json::from_str(text) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "failed to parse CDP request");
                let response = CdpResponse {
                    id: 0,
                    result: None,
                    error: Some(crate::protocol::CdpError {
                        code: -32700,
                        message: format!("Parse error: {e}"),
                    }),
                    session_id: None,
                };
                self.send_response(response).await?;
                return Ok(());
            }
        };

        let request_id = request.id.unwrap_or(0);
        let session_id = request.session_id.clone();

        debug!(
            id = request_id,
            method = %request.method,
            "dispatching CDP command"
        );

        // Dispatch to domain handler
        let response = match domains::dispatch(&request.method, request.params) {
            Ok(result) => CdpResponse {
                id: request_id,
                result: Some(result.unwrap_or(serde_json::json!({}))),
                error: None,
                session_id,
            },
            Err(cdp_error) => CdpResponse {
                id: request_id,
                result: None,
                error: Some(cdp_error),
                session_id,
            },
        };

        self.send_response(response).await
    }

    /// Send a CDP response to the client.
    async fn send_response(&mut self, response: CdpResponse) -> anyhow::Result<()> {
        let text = serde_json::to_string(&response)?;
        debug!(text = %text, "sending CDP response");
        self.sink
            .send(tungstenite::Message::Text(text.into()))
            .await?;
        Ok(())
    }

    /// Send a CDP event to the client.
    pub async fn send_event(&mut self, event: CdpEvent) -> anyhow::Result<()> {
        let text = serde_json::to_string(&event)?;
        debug!(text = %text, "sending CDP event");
        self.sink
            .send(tungstenite::Message::Text(text.into()))
            .await?;
        Ok(())
    }

    /// Get the session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get the target ID (if attached).
    pub fn target_id(&self) -> Option<&str> {
        self.target_id.as_deref()
    }
}
