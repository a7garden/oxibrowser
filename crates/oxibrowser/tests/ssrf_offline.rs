//! Offline SSRF tests — exercise the `fetch` subcommand against loopback /
//! private targets to verify the SSRF filter blocks them, without requiring an
//! internet connection or any `#[ignore]` flag.
//!
//! Run with: `cargo test -p oxibrowser --test ssrf_offline`
//!
//! These tests run in CI because they hit only loopback addresses, which are
//! guaranteed to be available. They complement the existing `tests/cli.rs` and
//! `tests/integration.rs` suites, which need external connectivity and are
//! marked `#[ignore]`.
//!
//! ## Why no mock-server fetch test?
//!
//! The `fetch` subcommand does not expose an `--allow-private-ips` flag (only
//! `serve` does, see `main.rs` `Commands::Serve`). Wiremock binds to
//! `127.0.0.1`, which the SSRF filter must reject — so a happy-path fetch
//! against a wiremock instance cannot work through the CLI without changing
//! the public CLI surface. The mock-server helpers in `common/mod.rs` are
//! kept so that future code paths (e.g. a future `fetch --allow-private-ips`,
//! or programmatic tests against `oxibrowser_core::Browser`) can use them.

mod common;

use std::process::Command;

/// Path to the built binary (works in any profile — debug or release).
fn oxibrowser() -> String {
    env!("CARGO_BIN_EXE_oxibrowser").to_string()
}

/// Invokes the CLI in JSON mode and parses stdout as a `CliResponse` envelope.
fn run_cli_json(args: &[&str]) -> serde_json::Value {
    let output = Command::new(oxibrowser())
        .args(args)
        .output()
        .expect("failed to execute oxibrowser");
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "expected JSON stdout from `oxibrowser {}`, got parse error {e}: {stdout}",
            args.join(" ")
        )
    })
}

/// The SSRF filter must block a literal IPv4 loopback address. The filter
/// defaults to enabled (no `--allow-private-ips` flag is exposed on `fetch`,
/// only on `serve`), so a connection attempt to `127.0.0.1` must be rejected
/// before any network I/O occurs.
///
/// Routing path under test: `Commands::Fetch` → `run_fetch` → `fetch_direct`
/// → `Browser::new_page` → `Session::navigate` → `HttpClient::fetch` →
/// `HttpClient::check_ssrf` → `IpFilter::is_hostname_allowed("127.0.0.1")`
/// (parsed as IP, matched against `127.0.0.0/8`) → rejected.
///
/// This proves the integration is wired end-to-end through the CLI binary, not
/// just the unit-tested filter.
#[test]
fn test_ssrf_blocks_loopback_ipv4_via_cli() {
    let resp = run_cli_json(&["fetch", "http://127.0.0.1:1/", "--json"]);

    assert_eq!(
        resp["ok"], false,
        "SSRF filter must reject 127.0.0.1, got {resp}"
    );
    // Verify the failure mode is the SSRF filter, not a connection-refused /
    // timeout. The runtime error message must mention the SSRF block.
    let err = resp["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("SSRF") || err.to_lowercase().contains("blocked"),
        "expected SSRF-blocked error message, got error={err:?} (full response: {resp})"
    );
}

/// The mock-server helpers in `common` start a wiremock instance bound to
/// `127.0.0.1`. This test ensures the helpers stay usable (and would catch a
/// regression if `mock_html_server` ever returned a malformed URL) by asserting
/// the CLI rejects the loopback URL exactly as `test_ssrf_blocks_loopback_ipv4_via_cli`
/// does. Keeping the helpers exercised by at least one test prevents drift.
#[tokio::test]
async fn test_ssrf_blocks_mock_server_address() {
    let (_server, url) = common::mock_html_server(
        "<html><head><title>Test Page</title></head><body>Hello World</body></html>",
    )
    .await;

    let resp = run_cli_json(&["fetch", &url, "--json"]);

    assert_eq!(
        resp["ok"], false,
        "SSRF filter must reject loopback mock server URL {url}, got {resp}"
    );
}