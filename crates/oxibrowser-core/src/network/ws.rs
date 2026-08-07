//! WebSocket connection task + wire types for the JS WebSocket API.
//!
//! One background tokio task runs `run_ws_connection` per JS `WebSocket`.
//! Commands arrive on a per-socket tokio channel; events flow back on a single
//! shared std channel (id-keyed), drained by `drain_ws_events` on the JS thread.

use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

/// Outbound command from the session bridge to a socket task.
#[derive(Debug)]
pub enum WsCmd {
    Send(WsData),
    Close {
        code: Option<u16>,
        reason: Option<String>,
    },
}

/// Payload for a `Send`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsData {
    Text(String),
    Binary(Vec<u8>),
}

/// Inbound event from a socket task to the JS thread (id-keyed routing).
#[derive(Debug, PartialEq)]
pub enum WsEvent {
    Open {
        id: u64,
        protocol: String,
        extensions: String,
    },
    Message {
        id: u64,
        data: WsData,
    },
    Close {
        id: u64,
        code: u16,
        reason: String,
        was_clean: bool,
    },
    Error {
        id: u64,
        message: String,
    },
}

/// The per-socket background task body. Owns the live `WebSocketStream`.
///
/// Emits `Open` on handshake success, `Message` for each inbound text/binary
/// frame, `Close` (with `was_clean`) on a clean shutdown, and `Error`+`Close`
/// (code 1006, `was_clean: false`) on connect failure / timeout / read error.
pub async fn run_ws_connection(
    id: u64,
    url: String,
    protocols: Vec<String>,
    mut cmd_rx: tokio::sync::mpsc::Receiver<WsCmd>,
    event_tx: std::sync::mpsc::Sender<WsEvent>,
) {
    let mut req = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => {
            fail(id, &event_tx, format!("invalid url: {e}"));
            return;
        }
    };
    if !protocols.is_empty() {
        let _ = req.headers_mut().insert(
            "sec-websocket-protocol",
            protocols
                .join(", ")
                .parse()
                .unwrap_or_else(|_| " ".parse().unwrap()),
        );
    }

    let connect = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(req),
    );
    let ws_stream = match connect.await {
        Ok(Ok((ws, resp))) => {
            let protocol = resp
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let _ = event_tx.send(WsEvent::Open {
                id,
                protocol,
                extensions: String::new(),
            });
            ws
        }
        Ok(Err(e)) => {
            fail(id, &event_tx, format!("connect failed: {e}"));
            return;
        }
        Err(_) => {
            fail(id, &event_tx, "connect timeout".into());
            return;
        }
    };

    let (mut sink, mut stream) = ws_stream.split();

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(WsCmd::Send(WsData::Text(t))) => {
                    if sink.send(Message::text(t)).await.is_err() {
                        fail(id, &event_tx, "send failed".into());
                        return;
                    }
                }
                Some(WsCmd::Send(WsData::Binary(b))) => {
                    if sink.send(Message::binary(b)).await.is_err() {
                        fail(id, &event_tx, "send failed".into());
                        return;
                    }
                }
                Some(WsCmd::Close { code, reason }) => {
                    let code = code.unwrap_or(1000);
                    let reason_str = reason.unwrap_or_default();
                    let cf = CloseFrame {
                        code: code.into(),
                        reason: reason_str.clone().into(),
                    };
                    let _ = sink.send(Message::Close(Some(cf))).await;
                    let _ = sink.close().await;
                    let _ = event_tx.send(WsEvent::Close {
                        id,
                        code,
                        reason: reason_str,
                        was_clean: true,
                    });
                    return;
                }
                None => return,
            },
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let _ = event_tx.send(WsEvent::Message {
                        id,
                        data: WsData::Text(t.to_string()),
                    });
                }
                Some(Ok(Message::Binary(b))) => {
                    let _ = event_tx.send(WsEvent::Message {
                        id,
                        data: WsData::Binary(b.to_vec()),
                    });
                }
                Some(Ok(Message::Close(c))) => {
                    let (code, reason) = c
                        .map(|cf| (u16::from(cf.code), cf.reason.to_string()))
                        .unwrap_or((1000, String::new()));
                    let _ = event_tx.send(WsEvent::Close {
                        id,
                        code,
                        reason,
                        was_clean: true,
                    });
                    return;
                }
                Some(Ok(_)) => { /* ping/pong ignored */ }
                Some(Err(e)) => {
                    fail(id, &event_tx, format!("read error: {e}"));
                    return;
                }
                None => {
                    let _ = event_tx.send(WsEvent::Close {
                        id,
                        code: 1006,
                        reason: String::new(),
                        was_clean: false,
                    });
                    return;
                }
            }
        }
    }
}

/// Emit `Error` then `Close(1006, "", false)` — the connect-failure path.
fn fail(id: u64, event_tx: &std::sync::mpsc::Sender<WsEvent>, message: String) {
    let _ = event_tx.send(WsEvent::Error { id, message });
    let _ = event_tx.send(WsEvent::Close {
        id,
        code: 1006,
        reason: String::new(),
        was_clean: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    async fn echo_server(port_tx: std::sync::mpsc::Sender<u16>) {
        // Bind an ephemeral port, publish it, then accept one connection.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        port_tx.send(listener.local_addr().unwrap().port()).unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(msg)) = ws.next().await {
            if msg.is_close() {
                break;
            }
            if msg.is_text() || msg.is_binary() {
                ws.send(msg).await.unwrap();
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_ws_connection_echo_roundtrip() {
        let (event_tx, event_rx) = std::sync::mpsc::channel::<WsEvent>();
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<WsCmd>(8);

        // Server binds + publishes its port before we connect — no race.
        let (port_tx, port_rx) = std::sync::mpsc::channel();
        let server = tokio::spawn(echo_server(port_tx));
        let port = port_rx.recv().unwrap();
        let url = format!("ws://127.0.0.1:{port}");
        let conn = tokio::spawn(run_ws_connection(1, url, vec![], cmd_rx, event_tx));

        // Open
        let open = event_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(open, WsEvent::Open { id: 1, .. }), "got {open:?}");

        // echo
        cmd_tx
            .send(WsCmd::Send(WsData::Text("ping".into())))
            .await
            .unwrap();
        let msg = event_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            msg,
            WsEvent::Message {
                id: 1,
                data: WsData::Text("ping".into())
            }
        );

        // close
        cmd_tx
            .send(WsCmd::Close {
                code: Some(1000),
                reason: Some("bye".into()),
            })
            .await
            .unwrap();
        let close = event_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            matches!(close, WsEvent::Close { id: 1, code: 1000, ref reason, was_clean: true } if reason == "bye"),
            "got {close:?}"
        );

        conn.await.unwrap();
        server.abort();
    }
}
