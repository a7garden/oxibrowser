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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = BrowserConfig::default();
        assert!(config.user_agent.contains("OxiBrowser"));
        assert_eq!(config.default_timeout, Duration::from_secs(30));
        assert!(config.obey_robots, "default should obey robots.txt");
        assert_eq!(config.max_sessions, 10);
        assert_eq!(config.viewport_width, 1280);
        assert_eq!(config.viewport_height, 720);
        assert!(!config.accept_invalid_certs);
    }

    #[test]
    fn test_headless_config() {
        let config = BrowserConfig::headless();
        assert!(!config.enable_rendering, "headless should disable rendering");
        assert_eq!(config.viewport_width, 0, "headless should have zero viewport width");
        assert_eq!(config.viewport_height, 0, "headless should have zero viewport height");
    }

    #[test]
    fn test_automation_config() {
        let config = BrowserConfig::automation();
        assert!(!config.obey_robots, "automation should ignore robots.txt");
        assert_eq!(config.default_timeout, Duration::from_secs(60), "automation should have longer timeout");
        assert_eq!(config.connection_pool_size, 20);
    }
}
