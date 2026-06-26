//! Browser configuration.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::PathBuf;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Serde helpers for Duration ↔ seconds
// ---------------------------------------------------------------------------

mod duration_secs {
    use super::{Deserialize, Deserializer, Duration, Serializer};

    pub fn serialize<S: Serializer>(dur: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(dur.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}

// ---------------------------------------------------------------------------
// Default functions for serde(default = "...")
// ---------------------------------------------------------------------------

fn default_user_agent() -> String {
    // Chrome 149 macOS — must match the wreq Emulation::Chrome149 profile
    // (sec-ch-ua v=149, sec-ch-ua-platform "macOS") so transport and JS
    // navigator.userAgent agree. See network/client.rs emulation().
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36".to_string()
}

fn default_timeout_secs() -> Duration {
    Duration::from_secs(30)
}

fn default_true() -> bool {
    true
}

fn default_max_sessions() -> usize {
    10
}

fn default_viewport_width() -> u32 {
    1280
}

fn default_viewport_height() -> u32 {
    720
}

fn default_connection_pool_size() -> usize {
    10
}

fn default_js_timeout_ms() -> u64 {
    5000
}

fn default_js_max_recursion() -> usize {
    100
}

fn default_js_max_loop_iterations() -> u64 {
    100_000
}

fn default_js_max_stack_size() -> usize {
    1024
}

fn default_navigation_timeout_ms() -> u64 {
    30_000
}

// ---------------------------------------------------------------------------
// BrowserConfig
// ---------------------------------------------------------------------------

/// Configuration for a Browser instance.
///
/// Supports `Serialize`/`Deserialize` so Agent OS can embed it directly
/// in a TOML config file under `[browser.engine]`.
///
/// # TOML example
///
/// ```toml
/// [browser.engine]
/// user_agent = "MyBot/1.0"
/// obey_robots = false
/// js_timeout_ms = 10000
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// User-Agent string sent with requests.
    #[serde(default = "default_user_agent")]
    pub user_agent: String,

    /// Default page navigation timeout (in seconds for serialization).
    #[serde(default = "default_timeout_secs", with = "duration_secs")]
    pub default_timeout: Duration,

    /// Whether to obey robots.txt.
    #[serde(default = "default_true")]
    pub obey_robots: bool,

    /// Maximum number of concurrent sessions.
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,

    /// Viewport width for rendering (0 = no rendering).
    #[serde(default = "default_viewport_width")]
    pub viewport_width: u32,

    /// Viewport height for rendering (0 = no rendering).
    #[serde(default = "default_viewport_height")]
    pub viewport_height: u32,

    /// Enable offscreen rendering (requires servo feature).
    #[serde(default)]
    pub enable_rendering: bool,

    /// HTTP connection pool size.
    #[serde(default = "default_connection_pool_size")]
    pub connection_pool_size: usize,

    /// Accept invalid TLS certificates.
    #[serde(default)]
    pub accept_invalid_certs: bool,

    /// Enable SSRF protection (IP filter for private/internal IPs).
    /// Defaults to `true`. Set to `false` for testing or when CDP clients
    /// need to navigate to local services.
    #[serde(default = "default_true")]
    pub enable_ssrf_filter: bool,

    /// JS execution timeout in milliseconds.
    /// A single `evaluate()` call that runs longer than this will be aborted
    /// and the JS context will be reset.
    #[serde(default = "default_js_timeout_ms")]
    pub js_timeout_ms: u64,

    /// Maximum JS recursion depth (function call stack depth).
    /// Prevents infinite recursion like `function f() { f(); }`.
    #[serde(default = "default_js_max_recursion")]
    pub js_max_recursion: usize,

    /// Maximum JS loop iteration count.
    /// Prevents infinite loops like `while(true){}`.
    /// Set to `u64::MAX` for no limit.
    #[serde(default = "default_js_max_loop_iterations")]
    pub js_max_loop_iterations: u64,

    /// Maximum JS operand stack size.
    #[serde(default = "default_js_max_stack_size")]
    pub js_max_stack_size: usize,

    /// Navigation timeout in milliseconds (time to wait for page load).
    #[serde(default = "default_navigation_timeout_ms")]
    pub navigation_timeout_ms: u64,

    /// Cookie jar persistence file path. `None` = in-memory only (default).
    #[serde(default)]
    pub cookie_file: Option<PathBuf>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            user_agent: default_user_agent(),
            default_timeout: default_timeout_secs(),
            obey_robots: default_true(),
            max_sessions: default_max_sessions(),
            viewport_width: default_viewport_width(),
            viewport_height: default_viewport_height(),
            enable_rendering: false,
            connection_pool_size: default_connection_pool_size(),
            accept_invalid_certs: false,
            enable_ssrf_filter: default_true(),
            js_timeout_ms: default_js_timeout_ms(),
            js_max_recursion: default_js_max_recursion(),
            js_max_loop_iterations: default_js_max_loop_iterations(),
            js_max_stack_size: default_js_max_stack_size(),
            navigation_timeout_ms: default_navigation_timeout_ms(),
            cookie_file: None,
        }
    }
}

impl BrowserConfig {
    /// Create a minimal config with no rendering.
    pub fn headless() -> Self {
        Self {
            enable_rendering: false,
            ..Self::default()
        }
    }

    /// Create a config optimized for automation.
    pub fn automation() -> Self {
        Self {
            obey_robots: false,
            default_timeout: Duration::from_secs(60),
            connection_pool_size: 20,
            js_timeout_ms: 10_000,
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
        assert!(config.user_agent.contains("Chrome/149"));
        assert_eq!(config.default_timeout, Duration::from_secs(30));
        assert!(config.obey_robots, "default should obey robots.txt");
        assert_eq!(config.max_sessions, 10);
        assert_eq!(config.viewport_width, 1280);
        assert_eq!(config.viewport_height, 720);
        assert!(!config.accept_invalid_certs);
        assert_eq!(config.js_timeout_ms, 5000);
        assert_eq!(config.js_max_recursion, 100);
        assert_eq!(config.js_max_loop_iterations, 100_000);
        assert_eq!(config.js_max_stack_size, 1024);
        assert_eq!(config.navigation_timeout_ms, 30_000);
        assert!(
            config.cookie_file.is_none(),
            "default should have no cookie file"
        );
    }

    #[test]
    fn test_headless_config() {
        let config = BrowserConfig::headless();
        assert!(
            !config.enable_rendering,
            "headless should disable rendering"
        );
        assert_eq!(
            config.viewport_width, 1280,
            "headless should have default viewport width"
        );
        assert_eq!(
            config.viewport_height, 720,
            "headless should have default viewport height"
        );
    }

    #[test]
    fn test_automation_config() {
        let config = BrowserConfig::automation();
        assert!(!config.obey_robots, "automation should ignore robots.txt");
        assert_eq!(
            config.default_timeout,
            Duration::from_secs(60),
            "automation should have longer timeout"
        );
        assert_eq!(config.connection_pool_size, 20);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = BrowserConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let config2: BrowserConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config2.user_agent, config.user_agent);
        assert_eq!(config2.default_timeout, config.default_timeout);
        assert_eq!(config2.obey_robots, config.obey_robots);
        assert_eq!(config2.max_sessions, config.max_sessions);
        assert_eq!(config2.js_timeout_ms, config.js_timeout_ms);
        assert_eq!(config2.cookie_file, config.cookie_file);
    }

    #[test]
    fn test_config_partial_deserialize() {
        // Only override a few fields — the rest should be defaults
        let json = r#"{"obey_robots": false, "js_timeout_ms": 9999}"#;
        let config: BrowserConfig = serde_json::from_str(json).unwrap();
        assert!(!config.obey_robots);
        assert_eq!(config.js_timeout_ms, 9999);
        // Defaults preserved
        assert!(config.user_agent.contains("Chrome/149"));
        assert_eq!(config.max_sessions, 10);
    }

    #[test]
    fn test_config_empty_object_gives_defaults() {
        let json = "{}";
        let config: BrowserConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.user_agent, default_user_agent());
        assert_eq!(config.default_timeout, default_timeout_secs());
        assert!(config.obey_robots);
        assert_eq!(config.max_sessions, default_max_sessions());
    }
}
