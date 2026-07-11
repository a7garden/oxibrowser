//! Shared test helpers for offline integration tests.
//!
//! These helpers spin up a local `wiremock` HTTP server so fetch/SSRF/navigation
//! tests can run without any external network access. They are designed to be
//! used from any `tests/*.rs` file via `mod common;`.
//!
//! `mock_json_server` is kept alongside `mock_html_server` even when no
//! current caller exercises it — future fetch/navigation tests will need it
//! (see F-11 ticket). Without the allow below, CI's `cargo clippy
//! --workspace --all-targets -- -D warnings` would fail on the unused `pub fn`.
// Allow dead_code: helpers may be unused in any single test binary but are
// intended to be shared across the offline integration test suite.
#![allow(dead_code)]

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Start a mock HTTP server that serves a simple HTML page on `/`.
///
/// Returns the running [`MockServer`] (kept alive for the duration of the test;
/// dropping it stops the background task) and the fully-qualified URL clients
/// should hit (`{server_uri}/`).
pub async fn mock_html_server(html: &str) -> (MockServer, String) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(html)
                .insert_header("content-type", "text/html"),
        )
        .mount(&server)
        .await;
    let url = format!("{}/", server.uri());
    (server, url)
}

/// Start a mock server that returns JSON on `/`.
pub async fn mock_json_server(json: &str) -> (MockServer, String) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(json)
                .insert_header("content-type", "application/json"),
        )
        .mount(&server)
        .await;
    let url = format!("{}/", server.uri());
    (server, url)
}