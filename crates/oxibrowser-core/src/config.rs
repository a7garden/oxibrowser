//! Browser configuration.

use std::time::Duration;

/// Configuration for a Browser instance.
#[derive(Debug, Clone)]
pub struct BrowserConfig {
    /// User-Agent string sent with requests.
    pub user_agent: String,

    /// Default page navigation timeout.
    pub default_timeout: Duration,

    /// Whether to obey robots.txt.
    pub obey_robots: bool,

    /// Maximum number of concurrent sessions.
    pub max_sessions: usize,

    /// Viewport width for rendering (0 = no rendering).
    pub viewport_width: u32,

    /// Viewport height for rendering (0 = no rendering).
    pub viewport_height: u32,

    /// Enable offscreen rendering (requires servo feature).
    pub enable_rendering: bool,

    /// HTTP connection pool size.
    pub connection_pool_size: usize,

    /// Accept invalid TLS certificates.
    pub accept_invalid_certs: bool,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            user_agent: format!(
                "Mozilla/5.0 (OxiBrowser/0.1.0; +https://github.com/oxios/oxibrowser)"
            ),
            default_timeout: Duration::from_secs(30),
            obey_robots: true,
            max_sessions: 10,
            viewport_width: 1280,
            viewport_height: 720,
            enable_rendering: false,
            connection_pool_size: 10,
            accept_invalid_certs: false,
        }
    }
}

impl BrowserConfig {
    /// Create a minimal config with no rendering.
    pub fn headless() -> Self {
        Self {
            enable_rendering: false,
            viewport_width: 0,
            viewport_height: 0,
            ..Self::default()
        }
    }

    /// Create a config optimized for automation.
    pub fn automation() -> Self {
        Self {
            obey_robots: false,
            default_timeout: Duration::from_secs(60),
            connection_pool_size: 20,
            ..Self::default()
        }
    }
}
