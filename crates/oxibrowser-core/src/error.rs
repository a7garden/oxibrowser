//! Error types for oxibrowser-core.

use thiserror::Error;

/// Core error type.
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("navigation failed: {0}")]
    NavigationFailed(String),

    #[error("JavaScript execution error: {0}")]
    JsError(String),

    #[error("network error: {0}")]
    NetworkError(String),

    #[error("page error: {0}")]
    PageError(String),

    #[error("session error: {0}")]
    SessionError(String),

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("browser closed")]
    BrowserClosed,

    #[error("invalid URL: {0}")]
    InvalidUrl(String),

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
        CoreError::NetworkError(e.to_string())
    }
}
