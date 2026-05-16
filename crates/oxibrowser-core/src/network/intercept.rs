//! Paused request registry for Fetch domain request interception.
//!
//! Stores paused requests when Fetch.enable is active and a request matches a pattern.
//! The CDP client responds with continue/fail/fulfill, and the HTTP client applies it.
//!
//! This lives in `oxibrowser-core` (not cdp) to avoid circular dependencies.

use parking_lot::RwLock;
use std::collections::HashMap;
use tokio::sync::oneshot;

/// Intercepted response body.
#[derive(Debug, Clone)]
pub enum InterceptedBody {
    Bytes(Vec<u8>),
}

/// A synthetic HTTP response from Fetch.fulfillRequest.
#[derive(Debug, Clone)]
pub struct InterceptedResponse {
    pub status_code: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: InterceptedBody,
}

/// Action to take on a paused request.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum InterceptAction {
    /// Resume the request with optional modifications.
    Continue {
        url: Option<String>,
        method: Option<String>,
        headers: Vec<(String, String)>,
        post_data: Option<String>,
    },
    /// Fail the request with an error message.
    Fail { error_reason: String },
    /// Fulfill with a synthetic response (no actual network request).
    Fulfill {
        status_code: u16,
        status_text: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
}

/// A paused request awaiting a CDP client decision.
pub struct PausedRequest {
    /// Original request URL.
    pub url: String,
    /// HTTP method.
    pub method: String,
    /// Request headers.
    pub headers: Vec<(String, String)>,
    /// Resource type (Document, Script, XHR, etc.).
    pub resource_type: String,
    /// Channel to send the interception decision.
    pub tx: oneshot::Sender<InterceptAction>,
}

/// Thread-safe registry of paused requests.
/// Each request has a unique ID → PausedRequest mapping.
#[derive(Default)]
pub struct PausedRequestRegistry {
    requests: RwLock<HashMap<String, PausedRequest>>,
}

impl PausedRequestRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a new paused request.
    ///
    /// Returns `Err(())` if the requestId is already registered.
    pub fn insert(&self, request_id: String, request: PausedRequest) -> bool {
        let mut guard = self.requests.write();
        if guard.contains_key(&request_id) {
            return false;
        }
        guard.insert(request_id, request);
        true
    }

    /// Take (remove and return) a paused request by ID.
    pub fn take(&self, request_id: &str) -> Option<PausedRequest> {
        let mut guard = self.requests.write();
        guard.remove(request_id)
    }

    /// Remove a request without returning it.
    pub fn remove(&self, request_id: &str) {
        let mut guard = self.requests.write();
        guard.remove(request_id);
    }

    /// Get the number of pending requests.
    pub fn len(&self) -> usize {
        self.requests.read().len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.requests.read().is_empty()
    }
}

/// Shared registry type alias.
pub type SharedRegistry = std::sync::Arc<PausedRequestRegistry>;

/// Create a new shared registry.
pub fn shared_registry() -> SharedRegistry {
    std::sync::Arc::new(PausedRequestRegistry::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_take() {
        let registry = PausedRequestRegistry::new();
        let (tx, _rx) = oneshot::channel();
        let req = PausedRequest {
            url: "https://example.com".to_string(),
            method: "GET".to_string(),
            headers: vec![],
            resource_type: "Document".to_string(),
            tx,
        };

        assert!(registry.insert("req-1".to_string(), req));
        assert_eq!(registry.len(), 1);
        // Duplicate insert should fail
        let (tx2, _rx2) = oneshot::channel();
        let req2 = PausedRequest {
            url: "https://example.com".to_string(),
            method: "GET".to_string(),
            headers: vec![],
            resource_type: "Document".to_string(),
            tx: tx2,
        };
        assert!(!registry.insert("req-1".to_string(), req2));
        let taken = registry.take("req-1").unwrap();
        assert_eq!(taken.url, "https://example.com");
        assert_eq!(registry.len(), 0);
        assert!(registry.take("nonexistent").is_none());
    }

    #[test]
    fn test_remove() {
        let registry = PausedRequestRegistry::new();
        let (tx, _rx) = oneshot::channel();
        let req = PausedRequest {
            url: "https://example.com".to_string(),
            method: "GET".to_string(),
            headers: vec![],
            resource_type: "Document".to_string(),
            tx,
        };
        registry.insert("req-1".to_string(), req);
        registry.remove("req-1");
        assert!(registry.take("req-1").is_none());
    }
}
