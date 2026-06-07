//! Browser — the top-level browser instance.
//!
//! Owns sessions, the HTTP client, and global browser state.

use crate::browse_result::BrowseResult;
use crate::config::BrowserConfig;
use crate::error::{CoreError, Result};
use crate::event::BrowserEvent;
use crate::network::HttpClient;
use crate::network::cookie::CookieJar;
use crate::session::Session;
use crate::tab::Tab;
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::broadcast;
use tracing::{info, warn};

/// Unique browser instance ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BrowserId(u64);

impl BrowserId {
    pub(crate) fn next() -> Self {
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
/// cookie jar, storage, and pages.
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
    /// Number of active Tab sessions (not in the sessions vec).
    tab_count: Arc<AtomicUsize>,
    /// Shutdown signal — broadcast to all session holders.
    shutdown_tx: broadcast::Sender<()>,
    /// Lifecycle event stream — `subscribe_events()` for observers.
    ///
    /// 32-slot buffer is plenty: we emit ≤4 events per page load
    /// (NavigationStarted, optional WaitingForSelector, DocumentReady,
    /// optional ScreenshotCaptured). The agent drops oldest on overflow.
    event_tx: broadcast::Sender<BrowserEvent>,
}

impl Browser {
    /// Create a new Browser instance with the given config.
    #[tracing::instrument(skip(config), err)]
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
        // 32 slots = generous headroom; we emit ≤4 events per page load.
        let (event_tx, _) = broadcast::channel::<BrowserEvent>(32);

        let id = BrowserId::next();
        info!(id = %id, "browser created");

        Ok(Self {
            id,
            config,
            http_client,
            sessions: RwLock::new(Vec::new()),
            cookie_jar,
            closed: std::sync::atomic::AtomicBool::new(false),
            tab_count: Arc::new(AtomicUsize::new(0)),
            shutdown_tx,
            event_tx,
        })
    }

    /// Create a new browsing session.
    ///
    /// A session represents a browsing context group (cookie jar, session
    /// storage, navigation history).
    #[tracing::instrument(skip(self), fields(id = %self.id), err)]
    pub async fn new_session(&self) -> Result<Arc<tokio::sync::RwLock<Session>>> {
        self.ensure_open()?;

        // Check capacity: both CDP sessions and Tab sessions count.
        let total = self.sessions.read().len() + self.tab_count.load(Ordering::Relaxed);
        if total >= self.config.max_sessions {
            return Err(CoreError::SessionError(
                "maximum number of sessions reached".into(),
            ));
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

    /// One-shot: URL → content.
    ///
    /// Creates a temporary session, navigates to the URL, extracts the
    /// `BrowseResult`, and cleans up. Cookies persist across calls via
    /// the browser's shared cookie jar.
    ///
    /// This covers the 90% agent use case: "read this URL".
    #[tracing::instrument(skip(self), fields(id = %self.id), err)]
    pub async fn browse(&self, url: &str) -> Result<BrowseResult> {
        self.ensure_open()?;
        let session = self.new_session().await?;
        let mut s = session.write().await;
        s.navigate(url).await?;
        let result = match s.page() {
            Some(page) => BrowseResult::from_page(page),
            None => BrowseResult::empty(),
        };
        // Close the temporary session to free the slot.
        // Non-fatal — we already have the result.
        let _ = s.close().await;
        drop(s);
        Ok(result)
    }

    /// Open an interactive tab for agent use.
    ///
    /// Returns a `Tab` that is `Clone` and takes `&self` only — no lock
    /// management needed by the consumer.
    ///
    /// The session counts toward `max_sessions` but is not tracked for
    /// CDP cleanup — use `Tab::close()` to release the slot.
    ///
    /// The returned `Tab` is wired to this `Browser`'s event stream —
    /// navigation/wait/screenshot operations emit `BrowserEvent`s to
    /// subscribers of `subscribe_events()`.
    #[tracing::instrument(skip(self), fields(id = %self.id), err)]
    pub async fn new_tab(&self) -> Result<Tab> {
        self.ensure_open()?;

        // Check capacity against tracked sessions
        let session_count = self.sessions.read().len() + self.tab_count.load(Ordering::Relaxed);
        if session_count >= self.config.max_sessions {
            return Err(CoreError::SessionError(
                "maximum number of sessions reached".into(),
            ));
        }

        self.tab_count.fetch_add(1, Ordering::Relaxed);

        let session = Session::new(
            self.id,
            self.config.clone(),
            self.http_client.clone(),
            self.cookie_jar.clone(),
        )
        .await?;

        let tab_id = uuid::Uuid::new_v4();
        tracing::info!(
            session_count = self.sessions.read().len(),
            tab_id = %tab_id,
            "new tab created"
        );
        Ok(Tab::new_with_cleanup_and_events(
            session,
            self.tab_count.clone(),
            self.event_tx.clone(),
            tab_id,
        ))
    }

    /// Convenience: create a session and navigate to a URL.
    #[tracing::instrument(skip(self), fields(id = %self.id), err)]
    pub async fn new_page(&self, url: &str) -> Result<Arc<tokio::sync::RwLock<Session>>> {
        let session = self.new_session().await?;
        session.write().await.navigate(url).await?;
        Ok(session)
    }

    /// Close all sessions and shut down.
    #[tracing::instrument(skip(self), fields(id = %self.id), err)]
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

    /// Subscribe to browser lifecycle events.
    ///
    /// Observers (e.g. oxi-agent's `OxiBrowserEngine`) use this to forward
    /// events to the agent loop's `ToolExecutionUpdate` callback. The
    /// returned receiver can be safely dropped; new subscribers get their
    /// own queue. On overflow, the **oldest** undelivered event is dropped
    /// (broadcast semantics) — observers should treat `RecvError::Lagged`
    /// as a non-fatal signal that they fell behind, not as a hard error.
    pub fn subscribe_events(&self) -> broadcast::Receiver<BrowserEvent> {
        self.event_tx.subscribe()
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

    /// Remove closed sessions from the active session list.
    ///
    /// Called by CDP session handlers after a WebSocket disconnects
    /// so that the session slot is freed for new connections.
    pub fn cleanup_closed_sessions(&self) {
        let mut sessions = self.sessions.write();
        let removed = sessions
            .extract_if(.., |s| match s.try_read() {
                Ok(guard) => guard.is_closed(),
                Err(_) => false, // locked — keep it for now
            })
            .count();
        if removed > 0 {
            info!(
                removed,
                session_count = sessions.len(),
                "cleaned up closed sessions"
            );
        }
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
                assert!(
                    msg.contains("maximum number of sessions"),
                    "error should mention max sessions, got: {msg}"
                );
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

    #[tokio::test]
    async fn test_browser_browse_after_close_returns_error() {
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await.unwrap();
        browser.close().await.unwrap();

        let result = browser.browse("https://example.com").await;
        assert!(result.is_err(), "browse() after close should fail");
        assert!(
            matches!(result, Err(CoreError::BrowserClosed)),
            "error should be BrowserClosed"
        );
    }

    #[tokio::test]
    async fn test_browser_new_tab_creates_tab() {
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await.unwrap();
        let tab = browser.new_tab().await;
        assert!(tab.is_ok(), "new_tab() should create a tab");
        let tab = tab.unwrap();
        assert!(!tab.is_closed(), "new tab should not be closed");
    }

    #[tokio::test]
    async fn test_browser_new_tab_clonable() {
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await.unwrap();
        let tab = browser.new_tab().await.unwrap();
        let tab2 = tab.clone();
        assert!(!tab2.is_closed());
    }

    #[tokio::test]
    async fn test_browser_new_tab_after_close_returns_error() {
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await.unwrap();
        browser.close().await.unwrap();

        let result = browser.new_tab().await;
        assert!(result.is_err(), "new_tab() after close should fail");
        assert!(
            matches!(result, Err(CoreError::BrowserClosed)),
            "error should be BrowserClosed"
        );
    }

    #[tokio::test]
    async fn test_browser_new_tab_respects_max_sessions() {
        let mut config = BrowserConfig::headless();
        config.max_sessions = 2;
        let browser = Browser::new(config).await.unwrap();

        let _s1 = browser.new_session().await.unwrap();
        let _t1 = browser.new_tab().await.unwrap();
        let t2 = browser.new_tab().await;

        assert!(
            t2.is_err(),
            "exceeding max_sessions via new_tab should fail"
        );
    }

    #[tokio::test]
    async fn test_subscribe_events_returns_receiver() {
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await.unwrap();
        // Should not panic; multiple subscribers should be supported.
        let _rx1 = browser.subscribe_events();
        let _rx2 = browser.subscribe_events();
    }

    #[tokio::test]
    async fn test_emit_event_does_not_block_on_no_subscribers() {
        use crate::event::BrowserEvent;
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await.unwrap();
        // No subscribers — emit should silently succeed.
        // (Direct channel access; subscribers only added by tests below.)
        for i in 0..100 {
            let _ = browser.event_tx.send(BrowserEvent::NavigationStarted {
                tab_id: uuid::Uuid::nil(),
                url: format!("https://example.com/{i}"),
            });
        }
    }

    #[tokio::test]
    async fn test_emit_event_reaches_subscriber() {
        use crate::event::BrowserEvent;
        let config = BrowserConfig::headless();
        let browser = Browser::new(config).await.unwrap();
        let mut rx = browser.subscribe_events();

        // The Tab is what emits events; simulate that path here.
        let _ = browser.event_tx.send(BrowserEvent::NavigationStarted {
            tab_id: uuid::Uuid::nil(),
            url: "https://example.com".into(),
        });

        let event = rx.try_recv().expect("subscriber should receive event");
        match event {
            BrowserEvent::NavigationStarted { url, .. } => {
                assert_eq!(url, "https://example.com");
            }
            other => panic!("expected NavigationStarted, got {other:?}"),
        }
    }
}
