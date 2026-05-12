//! Real-website integration tests.
//!
//! These tests require an active internet connection.
//! Run with: `cargo test --test integration -- --ignored`
//!
//! Note: assertions are intentionally lenient since external sites may change.

use oxibrowser_core::Browser;
use oxibrowser_core::config::BrowserConfig;

#[tokio::test]
#[ignore]
async fn test_fetch_example_com() {
    let browser = Browser::new(BrowserConfig::headless()).await.unwrap();
    let session = browser.new_page("https://example.com").await.unwrap();

    let guard = session.read().await;
    let page = guard.page().expect("page should be loaded");
    let title = page.title().expect("should have title");
    assert!(
        title.to_lowercase().contains("example"),
        "title: {title}"
    );
    drop(guard);

    browser.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn test_extract_links_from_example() {
    let browser = Browser::new(BrowserConfig::headless()).await.unwrap();
    let session = browser.new_page("https://example.com").await.unwrap();

    let guard = session.read().await;
    let page = guard.page().expect("page should be loaded");
    let doc = page.root_frame().document();
    let links = doc.query_selector_all("a");
    assert!(!links.is_empty(), "should have at least one link");

    // Verify at least some links have href attributes
    let mut href_count = 0;
    for link_id in &links {
        if let Some(node) = doc.get_node(*link_id) {
            if let Some(_href) = node.href() {
                href_count += 1;
            }
        }
    }
    assert!(href_count > 0, "should have at least one link with href");
    drop(guard);

    browser.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn test_js_document_query_on_real_page() {
    let browser = Browser::new(BrowserConfig::headless()).await.unwrap();
    let session = browser.new_page("https://example.com").await.unwrap();

    // Evaluate document.title via JS
    let result = {
        let mut guard = session.write().await;
        guard.evaluate_js("document.title").await.unwrap()
    };
    assert!(result.is_ok());
    if let Some(val) = &result.value {
        let title = val.as_str().unwrap_or("");
        assert!(
            title.to_lowercase().contains("example"),
            "JS document.title: {title}"
        );
    }

    // Evaluate document.querySelectorAll('a').length via JS
    let result = {
        let mut guard = session.write().await;
        guard
            .evaluate_js("document.querySelectorAll('a').length")
            .await
            .unwrap()
    };
    assert!(result.is_ok(), "querySelectorAll should succeed");

    drop(session);
    browser.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn test_markdown_conversion() {
    let browser = Browser::new(BrowserConfig::headless()).await.unwrap();
    let session = browser.new_page("https://example.com").await.unwrap();

    let guard = session.read().await;
    let page = guard.page().expect("page should be loaded");
    let md = page.to_markdown();
    assert!(!md.is_empty(), "markdown should not be empty");
    // example.com has known content — be lenient
    assert!(
        md.to_lowercase().contains("example") || md.to_lowercase().contains("domain"),
        "markdown should contain page text, got: {}",
        md.chars().take(200).collect::<String>()
    );
    drop(guard);

    browser.close().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn test_httpbin_get() {
    // httpbin.org/get returns JSON with request info
    let browser = Browser::new(BrowserConfig::headless()).await.unwrap();
    let session = browser.new_page("https://httpbin.org/get").await.unwrap();

    let guard = session.read().await;
    let page = guard.page().expect("page should be loaded");
    let content = page.content();
    assert!(
        content.contains("origin") || content.contains("url"),
        "httpbin /get should return JSON, got: {}",
        content.chars().take(200).collect::<String>()
    );
    drop(guard);

    browser.close().await.unwrap();
}
