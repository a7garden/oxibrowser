//! Error types for oxibrowser-core.

use thiserror::Error;

/// Core error type.
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("navigation failed: {0}")]
    NavigationFailed(String),

    #[error("network error: {0}")]
    NetworkError(String),

    #[error("DNS resolution failed: {0}")]
    DnsError(String),

    #[error("connection timeout: {0}")]
    ConnectionTimeout(String),

    #[error("HTTP {status}: {message}")]
    HttpError { status: u16, message: String },

    #[error("JavaScript evaluation error: {0}")]
    JsError(String),

    #[error("JS execution timed out after {0}ms")]
    JsTimeout(u64),

    #[error("JS runtime limit exceeded: {0}")]
    JsRuntimeLimit(String),

    #[error("page not loaded")]
    PageNotLoaded,

    #[error("page error: {0}")]
    PageError(String),

    #[error("session error: {0}")]
    SessionError(String),

    #[error("session closed")]
    SessionClosed,

    #[error("browser closed")]
    BrowserClosed,

    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("DOM error: {0}")]
    DomError(String),
}

/// Convenience Result alias.
pub type Result<T> = std::result::Result<T, CoreError>;

impl From<url::ParseError> for CoreError {
    fn from(e: url::ParseError) -> Self {
        CoreError::InvalidUrl(e.to_string())
    }
}

impl From<reqwest::Error> for CoreError {
    fn from(e: reqwest::Error) -> Self {
        // Classify reqwest errors into more specific variants
        if e.is_timeout() {
            return CoreError::ConnectionTimeout(e.to_string());
        }
        if e.is_connect() {
            // Check for DNS-like errors
            let msg = e.to_string();
            if msg.contains("dns")
                || msg.contains("resolve")
                || msg.contains("getaddrinfo")
                || msg.contains("Name or service not known")
                || msg.contains("nodename nor servname")
            {
                return CoreError::DnsError(msg);
            }
            return CoreError::NetworkError(msg);
        }
        CoreError::NetworkError(e.to_string())
    }
}
