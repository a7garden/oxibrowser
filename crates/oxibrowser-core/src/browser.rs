//! Browser — the top-level browser instance.
//!
//! Mirrors Lightpanda's `Browser.zig`: owns sessions, the HTTP client,
//! and global browser state.

use crate::config::BrowserConfig;
use crate::error::{CoreError, Result};
use crate::network::cookie::CookieJar;
use crate::network::HttpClient;
use crate::session::Session;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{info, warn};

/// Unique browser instance ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BrowserId(u64);

impl BrowserId {
    fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl std::fmt::Display for BrowserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "browser-{}", self.0)
    }
}

/// The top-level browser instance.
///
/// A Browser can hold multiple Sessions (browsing contexts), each with its own
/// cookie jar, storage, and pages. In Lightpanda terms, this is the `Browser`
/// struct that owns the JS environment, HTTP client, and session pool.
pub struct Browser {
    /// Unique ID.
    id: BrowserId,
    /// Configuration.
    config: BrowserConfig,
    /// Shared HTTP client.
    http_client: Arc<HttpClient>,
    /// Active sessions.
    sessions: RwLock<Vec<Arc<tokio::sync::RwLock<Session>>>>,
    /// Global cookie jar (shared across sessions by default).
    cookie_jar: Arc<RwLock<CookieJar>>,
    /// Whether the browser has been closed.
    closed: std::sync::atomic::AtomicBool,
    /// Shutdown signal — broadcast to all session holders.
    shutdown_tx: broadcast::Sender<()>,
}

impl Browser {
    /// Create a new Browser instance with the given config.
    pub async fn new(config: BrowserConfig) -> Result<Self> {
        let cookie_jar = if let Some(ref path) = config.cookie_file {
            match CookieJar::load_from_file(path) {
                Ok(jar) => {
                    info!(path = %path.display(), "loaded cookies from file");
                    jar
                }
                Err(e) => {
                    // File missing or invalid is not fatal — start with empty jar
                    info!(
                        path = %path.display(),
                        error = %e,
                        "could not load cookie file, starting with empty jar"
                    );
                    CookieJar::new()
                }
            }
        } else {
            CookieJar::new()
        };

        let cookie_jar = Arc::new(RwLock::new(cookie_jar));
        let http_client = Arc::new(HttpClient::new(&config, cookie_jar.clone())?);
        let (shutdown_tx, _) = broadcast::channel::<()>(1);

        let id = BrowserId::next();
        info!(id = %id, "browser created");

        Ok(Self {
            id,
            config,
            http_client,
            sessions: RwLock::new(Vec::new()),
            cookie_jar,
            closed: std::sync::atomic::AtomicBool::new(false),
            shutdown_tx,
        })
    }

    /// Create a new browsing session.
    ///
    /// A session represents a browsing context group (cookie jar, session
    /// storage, navigation history). Similar to Lightpanda's `Session.zig`.
    pub async fn new_session(&self) -> Result<Arc<tokio::sync::RwLock<Session>>> {
        self.ensure_open()?;

        // Check capacity under a short-lived read lock, then create the session
        // *outside* any lock to avoid holding a parking_lot::RwLock across .await.
        {
            let sessions = self.sessions.read();
            if sessions.len() >= self.config.max_sessions {
                return Err(CoreError::SessionError(
                    "maximum number of sessions reached".into(),
                ));
            }
        }

        let session = Session::new(
            self.id,
            self.config.clone(),
            self.http_client.clone(),
            self.cookie_jar.clone(),
        )
        .await?;

        let session = Arc::new(tokio::sync::RwLock::new(session));
        self.sessions.write().push(session.clone());

        info!(
            session_count = self.sessions.read().len(),
            "new session created"
        );
        Ok(session)
    }

    /// Convenience: create a session and navigate to a URL.
    pub async fn new_page(&self, url: &str) -> Result<Arc<tokio::sync::RwLock<Session>>> {
        let _span = tracing::info_span!("new_page", browser = %self.id, url = %url).entered();
        let session = self.new_session().await?;
        session.write().await.navigate(url).await?;
        Ok(session)
    }

    /// Close all sessions and shut down.
    pub async fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(()); // Already closed
        }

        // Save cookies to disk if a cookie_file path is configured
        if let Some(ref path) = self.config.cookie_file {
            let jar = self.cookie_jar.read();
            if let Err(e) = jar.save_to_file(path) {
                warn!(path = %path.display(), error = %e, "failed to save cookies to file");
            } else {
                info!(path = %path.display(), "saved cookies to file");
            }
        }

        // Broadcast shutdown signal to all session holders
        let _ = self.shutdown_tx.send(());

        // Drain sessions while holding the lock, then drop the lock
        // before awaiting session.close() to avoid holding a sync lock across await.
        let sessions: Vec<_> = self.sessions.write().drain(..).collect();
        // Lock is released here (sessions Vec goes out of scope implicitly)
        for session in sessions {
            let mut s = session.write().await;
            if let Err(e) = s.close().await {
                warn!("error closing session: {e}");
            }
        }

        info!("browser closed");
        Ok(())
    }

    /// Get a receiver for the shutdown signal.
    ///
    /// This can be used to detect when `close()` is called on the browser,
    /// e.g., for graceful shutdown in long-running tasks.
    pub fn shutdown_rx(&self) -> broadcast::Receiver<()> {
        self.shutdown_tx.subscribe()
    }

    /// Get the browser ID.
    pub fn id(&self) -> BrowserId {
        self.id
    }

    /// Get the browser config.
    pub fn config(&self) -> &BrowserConfig {
        &self.config
    }

    /// Get the HTTP client.
    pub fn http_client(&self) -> &Arc<HttpClient> {
        &self.http_client
    }

    /// Get the global cookie jar.
    pub fn cookie_jar(&self) -> &Arc<RwLock<CookieJar>> {
        &self.cookie_jar
    }

    /// Get active sessions.
    pub fn sessions(&self) -> &RwLock<Vec<Arc<tokio::sync::RwLock<Session>>>> {
        &self.sessions
    }

    /// Whether the browser is still open.
    pub fn is_open(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            Err(CoreError::BrowserClosed)
        } else {
            Ok(())
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if !self.closed.load(Ordering::SeqCst) {
            warn!("browser dropped without explicit close");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_browser_new_default_config() {
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await;
        assert!(
            browser.is_ok(),
            "Browser::new() with headless config should succeed"
        );
        let browser = browser.unwrap();
        assert!(browser.is_open());
    }

    #[tokio::test]
    async fn test_browser_new_session_creates_session() {
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await.unwrap();
        let session = browser.new_session().await;
        assert!(session.is_ok(), "new_session() should create a session");
        assert_eq!(browser.sessions().read().len(), 1);
    }

    #[tokio::test]
    async fn test_browser_new_session_respects_max_sessions() {
        let mut config = BrowserConfig::headless();
        config.max_sessions = 2;
        let browser = Browser::new(config).await.unwrap();

        let _s1 = browser.new_session().await.unwrap();
        let _s2 = browser.new_session().await.unwrap();
        let s3 = browser.new_session().await;

        assert!(s3.is_err(), "exceeding max_sessions should return error");
        match s3 {
            Err(CoreError::SessionError(msg)) => {
                assert!(msg.contains("maximum number of sessions"),
                    "error should mention max sessions, got: {msg}");
            }
            Err(e) => panic!("wrong error type: {e:?}"),
            Ok(_) => panic!("should have failed"),
        }
    }

    #[tokio::test]
    async fn test_browser_close_marks_closed() {
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await.unwrap();
        assert!(browser.is_open());

        browser.close().await.unwrap();
        assert!(!browser.is_open(), "browser should be closed after close()");
    }

    #[tokio::test]
    async fn test_browser_close_twice_no_panic() {
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await.unwrap();

        browser.close().await.unwrap();
        // Second close should succeed without panicking
        browser.close().await.unwrap();
        assert!(!browser.is_open());
    }

    #[tokio::test]
    async fn test_browser_new_session_after_close_returns_error() {
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await.unwrap();
        browser.close().await.unwrap();

        let result = browser.new_session().await;
        assert!(result.is_err(), "new_session() after close should fail");
        assert!(
            matches!(result, Err(CoreError::BrowserClosed)),
            "error should be BrowserClosed"
        );
    }
}
