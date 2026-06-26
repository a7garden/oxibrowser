//! Level-1 stealth surface — runtime verification.
//!
//! Confirms the Chrome fingerprint surface (navigator.webdriver, plugins,
//! window.chrome, WebGL, userAgentData, permissions, platform coherence) is
//! actually visible to JS at runtime, derived consistently from the configured
//! UA. Runs fully offline via a `data:` URL (see `Session::navigate` data:
//! handling — no HTTP fetch).

use oxibrowser_core::Browser;
use oxibrowser_core::config::BrowserConfig;

const UA_WINDOWS_CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/131.0.6778.139 Safari/537.36";

/// A real desktop-Chrome Windows UA must produce: webdriver=false, 5 plugins,
/// 2 mimeTypes, window.chrome, WebGLRenderingContext, userAgentData.platform
/// "Windows", navigator.platform "Win32", and a permissions.query function.
/// Each branch short-circuits to a diagnostic so a failure names the exact gap.
#[tokio::test]
async fn stealth_surface_visible_to_js_windows() {
    let mut cfg = BrowserConfig::headless();
    cfg.user_agent = UA_WINDOWS_CHROME.to_string();
    let browser = Browser::new(cfg).await.unwrap();
    let session = browser
        .new_page("data:text/html,<html></html>")
        .await
        .unwrap();

    let expr = "(navigator.webdriver === false ? \
        (navigator.plugins.length === 5 ? \
          (navigator.mimeTypes.length === 2 ? \
            (typeof window.chrome === 'object' && window.chrome !== null ? \
              (typeof WebGLRenderingContext === 'function' ? \
                (typeof navigator.userAgentData === 'object' \
                   && navigator.userAgentData.platform === 'Windows' ? \
                  (navigator.platform === 'Win32' ? \
                    (typeof navigator.permissions === 'object' \
                       && typeof navigator.permissions.query === 'function' ? \
                      'ok' : 'perms') \
                    : 'platform:' + navigator.platform) \
                  : 'uad:' + (navigator.userAgentData ? navigator.userAgentData.platform : 'none')) \
                : 'nogl') \
              : 'nochrome') \
            : 'mimes:' + navigator.mimeTypes.length) \
          : 'plugins:' + navigator.plugins.length) \
        : 'webdriver:' + navigator.webdriver)";

    let result = {
        let mut guard = session.write().await;
        guard.evaluate_js(expr).await.unwrap()
    };
    drop(session);
    browser.close().await.unwrap();

    assert!(result.is_ok(), "eval failed: {:?}", result);
    let s = result
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("<none>");
    assert_eq!(s, "ok", "stealth surface gap on Windows profile: {s}");
}

/// Cross-property consistency: `navigator.platform` must agree with the OS
/// advertised in the UA for every OS family. A MacIntel + Direct3D11 (or
/// Win32 + macOS) combination is exactly the impossible-on-real-hardware
/// mismatch that correlating detectors score highest, so this guards the
/// single-source-of-truth invariant end to end.
#[tokio::test]
async fn stealth_platform_matches_ua_os() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/131.0",
            "Win32",
            "Windows",
        ),
        (
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/131.0",
            "MacIntel",
            "macOS",
        ),
        (
            "Mozilla/5.0 (X11; Linux x86_64) Chrome/131.0",
            "Linux x86_64",
            "Linux",
        ),
    ];

    for (ua, expected_platform, label) in cases {
        let mut cfg = BrowserConfig::headless();
        cfg.user_agent = ua.to_string();
        let browser = Browser::new(cfg).await.unwrap();
        let session = browser
            .new_page("data:text/html,<html></html>")
            .await
            .unwrap();

        let result = {
            let mut guard = session.write().await;
            guard.evaluate_js("navigator.platform").await.unwrap()
        };
        drop(session);
        browser.close().await.unwrap();

        assert!(result.is_ok(), "eval failed for {label}: {:?}", result);
        let s = result
            .value
            .as_ref()
            .and_then(|v| v.as_str())
            .unwrap_or("<none>");
        assert_eq!(
            s, *expected_platform,
            "navigator.platform should be {expected_platform} for {label} UA, got {s}"
        );
    }
}
