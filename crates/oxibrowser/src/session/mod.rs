//! Session REPL — stdin/stdout JSON interface for AI agents.
//!
//! Reads commands from stdin (one per line), executes them, and prints
//! JSON responses to stdout. This is the machine-friendly interface
//! for subprocess-based automation.

pub mod executor;
pub mod parser;
pub mod tab_manager;

use crate::output::CliResponse;
use tab_manager::TabManager;

/// Run the session REPL. Returns exit code.
pub async fn run_session() -> i32 {
    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = match oxibrowser_core::Browser::new(config).await {
        Ok(b) => b,
        Err(e) => {
            let resp = CliResponse::error(
                format!("browser init failed: {e}"),
                "RUNTIME_ERROR",
            );
            resp.print_json();
            return 1;
        }
    };

    let mut manager = TabManager::new();
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();

    loop {
        match lines.next() {
            Some(Ok(line)) => {
                let line = line.trim().to_string();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }

                let cmd = match parser::parse_session_command(&line) {
                    Ok(c) => c,
                    Err(e) => {
                        if e == "empty" {
                            continue;
                        }
                        let resp = CliResponse::error(e, "PARSE_ERROR");
                        resp.print_json();
                        continue;
                    }
                };

                // Handle exit/quit before executing
                if matches!(cmd, parser::SessionCommand::Exit) {
                    manager.close_all().await;
                    browser.close().await.ok();
                    let resp = CliResponse::success(serde_json::json!({ "exit": true }));
                    resp.print_json();
                    break;
                }

                let resp = executor::execute(cmd, &browser, &mut manager).await;
                resp.print_json();
            }
            Some(Err(e)) => {
                let resp = CliResponse::error(
                    format!("stdin read error: {e}"),
                    "IO_ERROR",
                );
                resp.print_json();
                break;
            }
            None => {
                // EOF — clean up and exit
                manager.close_all().await;
                browser.close().await.ok();
                break;
            }
        }
    }

    0
}

/// BufRead trait is needed for `.lines()` on StdinLock.
use std::io::BufRead;
