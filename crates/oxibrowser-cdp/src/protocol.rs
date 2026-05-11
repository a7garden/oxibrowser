//! CDP protocol types and message handling.

use serde::{Deserialize, Serialize};

/// A CDP request message.
#[derive(Debug, Deserialize)]
pub struct CdpRequest {
    /// Command ID.
    pub id: Option<u64>,
    /// Domain.method (e.g., "Page.navigate").
    pub method: String,
    /// Method parameters.
    pub params: Option<serde_json::Value>,
    /// Session ID (for multiplexing).
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
}

/// A CDP response message.
#[derive(Debug, Serialize)]
pub struct CdpResponse {
    /// Command ID (matches the request).
    pub id: u64,
    /// Result data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CdpError>,
    /// Session ID.
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// CDP error in a response.
#[derive(Debug, Serialize)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
}

/// A CDP event notification.
#[derive(Debug, Serialize)]
pub struct CdpEvent {
    /// Event name (e.g., "Page.loadEventFired").
    pub method: String,
    /// Event parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// Session ID.
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl CdpEvent {
    /// Create a new event.
    pub fn new(method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            method: method.into(),
            params: Some(params),
            session_id: None,
        }
    }
}

/// /json/version HTTP endpoint response.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonVersion {
    pub browser: String,
    pub protocol_version: String,
    pub user_agent: String,
    pub v8_version: String,
    pub webkit_version: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    pub web_socket_url: String,
}

impl JsonVersion {
    pub fn new(ws_url: String) -> Self {
        Self {
            browser: "OxiBrowser/0.1.0".into(),
            protocol_version: "1.3".into(),
            user_agent: "OxiBrowser/0.1.0".into(),
            v8_version: "0.1.0".into(),
            webkit_version: "0.1.0".into(),
            web_socket_url: ws_url,
        }
    }
}

/// /json list endpoint response item.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonTarget {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub target_type: String,
    pub url: String,
    pub web_socket_debugger_url: String,
}

/// CDP error codes.
pub mod error_codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const SERVER_ERROR: i64 = -32000;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cdp_request() {
        let json = r#"{"id":1,"method":"Page.navigate","params":{"url":"https://example.com"}}"#;
        let req: CdpRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, Some(1));
        assert_eq!(req.method, "Page.navigate");
        assert!(req.params.is_some());
        let params = req.params.unwrap();
        assert_eq!(params["url"], "https://example.com");
    }

    #[test]
    fn test_serialize_cdp_response() {
        let resp = CdpResponse {
            id: 42,
            result: Some(serde_json::json!({"key": "value"})),
            error: None,
            session_id: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"id\":42"));
        assert!(json.contains("\"key\""));
        // error should be omitted (skip_serializing_if)
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn test_serialize_cdp_event() {
        let event = CdpEvent::new("Page.loadEventFired", serde_json::json!({"timestamp": 1234}));
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"method\":\"Page.loadEventFired\""));
        assert!(json.contains("\"timestamp\""));
    }

    #[test]
    fn test_json_version_serialization() {
        let v = JsonVersion::new("ws://127.0.0.1:9222/ws".to_string());
        let json = serde_json::to_string(&v).unwrap();
        // Verify camelCase field names
        assert!(json.contains("\"protocolVersion\""));
        assert!(json.contains("\"userAgent\""));
        assert!(json.contains("webSocketDebuggerUrl") || json.contains("webSocketUrl"),
            "should contain web socket url field");
        // Verify round-trip
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["browser"], "OxiBrowser/0.1.0");
        assert_eq!(parsed["protocolVersion"], "1.3");
    }
}
