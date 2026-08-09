//! CDP session — a single WebSocket connection for CDP communication.
//!
//! Manages a WebSocket connection, dispatches incoming CDP commands to
//! domain handlers, and sends responses and events back to the client.
//!
//! Each CDP session creates a corresponding Browser `Session` for page
//! interaction (navigation, DOM access, JS evaluation).

use crate::domains;
use crate::domains::DispatchContext;
use crate::event::{EventReceiver, EventSender, event_channel};
use crate::protocol::{CdpEvent, CdpRequest, CdpResponse};
use crate::server::MAX_CDP_MESSAGE_SIZE;
use futures::{SinkExt, StreamExt};
use oxibrowser_core::Browser;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite;
use tracing::{debug, error, info, warn};

/// A single CDP session over a WebSocket connection.
///
/// Holds a reference to the shared `Browser`, owns a `Session` that
/// represents the browsing context, and runs an event broadcaster that
/// forwards CDP events to the WebSocket client.
pub struct CdpSession {
    /// The WebSocket sink (for sending responses).
    sink: futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<hyper_util::rt::TokioIo<hyper::upgrade::Upgraded>>,
        tungstenite::Message,
    >,
    /// The WebSocket stream (for receiving commands).
    ws: futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<hyper_util::rt::TokioIo<hyper::upgrade::Upgraded>>,
    >,
    /// Session ID for this CDP connection.
    session_id: String,
    /// Target ID this session is attached to.
    #[allow(dead_code)]
    target_id: Option<String>,
    /// Shared browser instance (for creating new sessions, etc.).
    #[allow(dead_code)]
    browser: Arc<Browser>,
    /// Browser session for page interaction.
    session: Arc<RwLock<oxibrowser_core::session::Session>>,
    /// Event sender (cloned into DispatchContext for domain handlers).
    event_sender: EventSender,
    /// Registry of paused Fetch requests (shared via DispatchContext).
    fetch_registry: oxibrowser_core::network::SharedRegistry,
    /// Shared dialog-resolution gate (for Page.handleJavaScriptDialog).
    dialog_gate: oxibrowser_core::js::DialogGate,
    /// Event receiver (drained by background task).
    event_receiver: Option<EventReceiver>,
    /// Shutdown signal for the CoreEvent drainer task — sent when `run`
    /// ends so the drainer exits promptly (not just on channel disconnect).
    core_shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl CdpSession {
    /// Create a new CDP session wrapping a WebSocket stream.
    ///
    /// This also creates a new Browser `Session` for page interaction
    /// and an event broadcaster for CDP event publishing.
    #[tracing::instrument(skip(ws_stream, browser), err)]
    pub async fn new(
        ws_stream: tokio_tungstenite::WebSocketStream<
            hyper_util::rt::TokioIo<hyper::upgrade::Upgraded>,
        >,
        browser: Arc<Browser>,
    ) -> anyhow::Result<Self> {
        let (sink, ws) = ws_stream.split();
        let session_id = format!("session-{}", uuid::Uuid::new_v4());

        // Create a browser session for this CDP connection
        let session = browser.new_session().await?;

        let (event_sender, event_receiver) = event_channel();
        // CoreEvent sink: the JS thread (in core) pushes neutral CoreEvents
        // onto this channel; the drainer task translates them into CDP events.
        let (core_tx, core_rx) = std::sync::mpsc::channel::<oxibrowser_core::js::CoreEvent>();
        {
            let mut s = session.write().await;
            s.set_event_sink(core_tx);
        }
        // Spawn a drainer that pumps CoreEvents into CDP events. Exits when the
        // core sender drops (session/JS-thread teardown) OR when the shutdown
        // oneshot fires (run() ending) — whichever comes first — for a prompt,
        // deterministic exit instead of relying solely on disconnect.
        let (core_shutdown_tx, mut core_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let drain_events = event_sender.clone();
        tokio::spawn(async move {
            loop {
                // Drain everything currently queued without blocking.
                loop {
                    match core_rx.try_recv() {
                        Ok(ev) => crate::core_event::emit_core_event(&drain_events, ev),
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    }
                }
                // Wait for a new event tick or a shutdown signal.
                tokio::select! {
                    _ = &mut core_shutdown_rx => return,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
                }
            }
        });
        // Clone the shared dialog gate so Page.handleJavaScriptDialog can
        // resolve a pending dialog without acquiring the session lock.
        let dialog_gate = session.read().await.dialog_gate();

        info!(session_id = %session_id, "CDP session created");

        Ok(Self {
            ws,
            sink,
            session_id,
            target_id: None,
            browser,
            session,
            dialog_gate,
            event_sender,
            fetch_registry: oxibrowser_core::network::intercept::shared_registry(),
            event_receiver: Some(event_receiver),
            core_shutdown: Some(core_shutdown_tx),
        })
    }
    /// Run the message dispatch loop.
    ///
    /// Reads commands from the WebSocket, dispatches each **concurrently**
    /// (so a long-running command — e.g. `Runtime.evaluate` blocked on a
    /// dialog — cannot stall event forwarding or other commands), and forwards
    /// CDP responses + events to the client.
    #[tracing::instrument(skip(self), fields(session_id = %self.session_id), err)]
    pub async fn run(mut self) -> anyhow::Result<()> {
        info!(session_id = %self.session_id, "CDP session started");

        // Take the event receiver out — drained by this loop's select!.
        let mut event_rx = self
            .event_receiver
            .take()
            .ok_or_else(|| anyhow::anyhow!("event_receiver must be present at session start"))?;

        // Take the CoreEvent drainer shutdown signal so we can fire it when the
        // dispatch loop ends (prompt, deterministic drainer exit).
        let core_shutdown = self.core_shutdown.take();

        // Command responses flow back from spawned dispatch tasks through this
        // channel, keeping the select! free to poll ws input, events, and
        // responses concurrently.
        let (response_tx, mut response_rx) = tokio::sync::mpsc::unbounded_channel::<CdpResponse>();

        loop {
            tokio::select! {
                // Incoming CDP commands
                msg = self.ws.next() => {
                    match msg {
                        Some(Ok(tungstenite::Message::Text(text))) => {
                            debug!(text = %text, "received CDP message");
                            let ctx = DispatchContext {
                                session: self.session.clone(),
                                events: self.event_sender.clone(),
                                fetch_registry: self.fetch_registry.clone(),
                                dialog_gate: self.dialog_gate.clone(),
                            };
                            let response_tx = response_tx.clone();
                            // Spawn so dispatch never blocks the loop — a
                            // blocking evaluate (e.g. alert()) still lets
                            // Page.handleJavaScriptDialog be received & run.
                            tokio::spawn(async move {
                                let response = dispatch_command(text.to_string(), &ctx).await;
                                let _ = response_tx.send(response);
                            });
                        }
                        Some(Ok(tungstenite::Message::Close(_))) => {
                            info!(session_id = %self.session_id, "WebSocket closed by client");
                            break;
                        }
                        Some(Ok(tungstenite::Message::Ping(data))) => {
                            self.sink.send(tungstenite::Message::Pong(data)).await?;
                        }
                        Some(Ok(_)) => {
                            // Binary, Pong, Frame — ignore
                        }
                        Some(Err(e)) => {
                            error!(error = %e, "WebSocket read error");
                            break;
                        }
                        None => {
                            info!(session_id = %self.session_id, "WebSocket stream ended");
                            break;
                        }
                    }
                }
                // Command responses from spawned dispatch tasks
                response = response_rx.recv() => {
                    if let Some(response) = response
                        && let Err(e) = self.send_response(response).await
                    {
                        warn!(error = %e, "failed to send CDP response");
                        break;
                    }
                }
                // Outgoing CDP events (independent of dispatch — decoupled so
                // e.g. Page.javascriptDialogOpening reaches the client while a
                // blocking evaluate is still pending).
                event = event_rx.recv() => {
                    match event {
                        Some(event) => {
                            if let Err(e) = self.send_event(event).await {
                                warn!(error = %e, "failed to send CDP event");
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }

        // Drop our response sender; in-flight dispatch tasks finish and their
        // sends are dropped (channel receiver gone on return).
        drop(response_tx);
        // Signal the CoreEvent drainer to exit promptly (it also exits on
        // channel disconnect once the Session closes below).
        if let Some(tx) = core_shutdown {
            let _ = tx.send(());
        }

        // Close the underlying Session so is_closed() returns true.
        self.session.write().await.close().await.ok();

        // Free the session slot so new connections are not rejected
        // when max_sessions is reached.
        self.browser.cleanup_closed_sessions();

        info!(session_id = %self.session_id, "CDP session ended");
        Ok(())
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
    async fn send_event(&mut self, event: CdpEvent) -> anyhow::Result<()> {
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

/// Parse + dispatch a single CDP command text, returning the response.
///
/// Runs as a spawned task so dispatch never blocks the run loop's `select!`
/// (a long-running command like a dialog-blocked `Runtime.evaluate` must not
/// stall event forwarding or other commands).
async fn dispatch_command(text: String, ctx: &DispatchContext) -> CdpResponse {
    // Validate message size
    if text.len() > MAX_CDP_MESSAGE_SIZE {
        warn!(
            size = text.len(),
            max = MAX_CDP_MESSAGE_SIZE,
            "CDP message too large, dropping"
        );
        return CdpResponse {
            id: 0,
            result: None,
            error: Some(crate::protocol::CdpError {
                code: -32600,
                message: format!(
                    "Message too large: {} bytes (max {} bytes)",
                    text.len(),
                    MAX_CDP_MESSAGE_SIZE
                ),
            }),
            session_id: None,
        };
    }

    // Parse the CDP request
    let request: CdpRequest = match serde_json::from_str(&text) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "failed to parse CDP request");
            return CdpResponse {
                id: 0,
                result: None,
                error: Some(crate::protocol::CdpError {
                    code: -32700,
                    message: format!("Parse error: {e}"),
                }),
                session_id: None,
            };
        }
    };

    let request_id = request.id.unwrap_or(0);
    let session_id_for_response = request.session_id.clone();

    debug!(
        id = request_id,
        method = %request.method,
        "dispatching CDP command"
    );

    match domains::dispatch(&request.method, request.params, ctx).await {
        Ok(result) => CdpResponse {
            id: request_id,
            result: Some(result.unwrap_or(serde_json::json!({}))),
            error: None,
            session_id: session_id_for_response,
        },
        Err(cdp_error) => CdpResponse {
            id: request_id,
            result: None,
            error: Some(cdp_error),
            session_id: session_id_for_response,
        },
    }
}
