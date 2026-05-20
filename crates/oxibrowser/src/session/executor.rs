//! Session command executor.
//!
//! Takes a parsed `SessionCommand` and executes it against the browser
//! and tab manager, returning a `CliResponse`.

use crate::output::CliResponse;
use crate::session::parser::SessionCommand;
use crate::session::tab_manager::TabManager;
use oxibrowser_core::Browser;
use serde_json::Value;
use std::time::Instant;

/// Execute a session command and return a CliResponse.
pub async fn execute(
    cmd: SessionCommand,
    browser: &Browser,
    manager: &mut TabManager,
) -> CliResponse {
    let start = Instant::now();
    let result = execute_inner(cmd, browser, manager).await;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok((data, tab_id)) => {
            CliResponse::success_with_meta(data, tab_id, elapsed_ms)
        }
        Err(resp) => resp,
    }
}

type ExecResult = Result<(Value, Option<String>), CliResponse>;

async fn execute_inner(
    cmd: SessionCommand,
    browser: &Browser,
    manager: &mut TabManager,
) -> ExecResult {
    match cmd {
        // ---- Tab lifecycle ----
        SessionCommand::New => {
            let tab_id = manager.create_tab(browser).await.map_err(|e| {
                CliResponse::error(e, "RUNTIME_ERROR")
            })?;
            Ok((serde_json::json!({ "tab_id": tab_id }), Some(tab_id)))
        }

        SessionCommand::Close { tab_id } => {
            manager.close_tab(&tab_id).await.map_err(|e| {
                CliResponse::error(e, "RUNTIME_ERROR")
            })?;
            Ok((serde_json::json!({ "closed": tab_id }), None))
        }

        SessionCommand::CloseAll => {
            let count = manager.len();
            manager.close_all().await;
            Ok((serde_json::json!({ "closed_all": count }), None))
        }

        SessionCommand::List => {
            let tabs = manager.list();
            Ok((serde_json::json!({ "tabs": tabs }), None))
        }

        // ---- Navigation ----
        SessionCommand::Goto { tab_id, url, wait_selector, timeout_ms } => {
            let tab = get_tab(manager, &tab_id)?;
            let nav = tab.goto(&url).await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;

            // Wait for selector if requested
            if let Some(sel) = wait_selector {
                let timeout = timeout_ms.unwrap_or(5000);
                tab.wait_for(&sel, timeout).await.map_err(|e| {
                    CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
                })?;
            }

            let data = serde_json::json!({
                "url": nav.url,
                "title": nav.title,
                "status": nav.status,
            });
            Ok((data, Some(tab_id)))
        }

        SessionCommand::Back { tab_id } => {
            let tab = get_tab(manager, &tab_id)?;
            let nav = tab.back().await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({
                "url": nav.url,
                "title": nav.title,
                "status": nav.status,
            }), Some(tab_id)))
        }

        SessionCommand::Forward { tab_id } => {
            let tab = get_tab(manager, &tab_id)?;
            let nav = tab.forward().await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({
                "url": nav.url,
                "title": nav.title,
                "status": nav.status,
            }), Some(tab_id)))
        }

        SessionCommand::Reload { tab_id } => {
            let tab = get_tab(manager, &tab_id)?;
            let nav = tab.reload().await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({
                "url": nav.url,
                "title": nav.title,
                "status": nav.status,
            }), Some(tab_id)))
        }

        // ---- Interaction ----
        SessionCommand::Click { tab_id, selector } => {
            let tab = get_tab(manager, &tab_id)?;
            tab.click(&selector).await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({ "clicked": selector }), Some(tab_id)))
        }

        SessionCommand::Fill { tab_id, selector, value } => {
            let tab = get_tab(manager, &tab_id)?;
            tab.fill(&selector, &value).await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({
                "filled": selector,
                "value_length": value.len(),
            }), Some(tab_id)))
        }

        SessionCommand::Press { tab_id, key } => {
            let tab = get_tab(manager, &tab_id)?;
            tab.press(&key).await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({ "pressed": key }), Some(tab_id)))
        }

        SessionCommand::Type { tab_id, selector, text } => {
            let tab = get_tab(manager, &tab_id)?;
            tab.r#type(&selector, &text).await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({
                "typed": selector,
                "text_length": text.len(),
            }), Some(tab_id)))
        }

        SessionCommand::Select { tab_id, selector, value } => {
            let tab = get_tab(manager, &tab_id)?;
            tab.select_option(&selector, &value).await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({
                "selected": selector,
                "value": value,
            }), Some(tab_id)))
        }

        SessionCommand::Check { tab_id, selector } => {
            let tab = get_tab(manager, &tab_id)?;
            tab.check(&selector).await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({ "checked": selector }), Some(tab_id)))
        }

        SessionCommand::Uncheck { tab_id, selector } => {
            let tab = get_tab(manager, &tab_id)?;
            tab.uncheck(&selector).await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({ "unchecked": selector }), Some(tab_id)))
        }

        SessionCommand::Scroll { tab_id, dx, dy } => {
            let tab = get_tab(manager, &tab_id)?;
            tab.scroll(dx, dy).await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({ "scrolled": { "dx": dx, "dy": dy } }), Some(tab_id)))
        }

        // ---- JS evaluation ----
        SessionCommand::Eval { tab_id, expression, await_promise } => {
            let tab = get_tab(manager, &tab_id)?;
            let value = if await_promise {
                tab.evaluate_await(&expression).await.map_err(|e| {
                    CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
                })?
            } else {
                tab.evaluate(&expression).await.map_err(|e| {
                    CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
                })?
            };
            Ok((serde_json::json!({ "value": value }), Some(tab_id)))
        }

        // ---- Extraction ----
        SessionCommand::Extract {
            tab_id, selector, all, attrs, links, title, text, markdown, max_bytes
        } => {
            let tab = get_tab(manager, &tab_id)?;
            let content = tab.content().await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;

            let mut data = serde_json::Map::new();

            if title {
                data.insert("title".into(), Value::String(content.title.clone()));
            }
            if links {
                let hrefs = tab.query_all("a[href]").await.map_err(|e| {
                    CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
                })?;
                data.insert("links".into(), Value::Array(
                    hrefs.into_iter().map(Value::String).collect()
                ));
            }
            if text {
                data.insert("text".into(), Value::String(content.markdown.clone()));
            }
            if markdown {
                data.insert("markdown".into(), Value::String(content.markdown.clone()));
            }

            if let Some(ref sel) = selector {
                let matches = tab.query_all(sel).await.map_err(|e| {
                    CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
                })?;
                let requested_attrs: Vec<&str> = attrs
                    .as_deref()
                    .map(|a| a.split(',').map(|s| s.trim()).collect())
                    .unwrap_or_default();

                if all {
                    let items: Vec<Value> = matches.into_iter().map(|t| {
                        if requested_attrs.is_empty() {
                            Value::String(t)
                        } else {
                            serde_json::json!({ "text": t })
                        }
                    }).collect();
                    data.insert("selector".into(), Value::String(sel.clone()));
                    data.insert("count".into(), serde_json::json!(items.len()));
                    data.insert("items".into(), Value::Array(items));
                } else {
                    let m = matches.first().cloned().unwrap_or_default();
                    data.insert("selector".into(), Value::String(sel.clone()));
                    data.insert("match".into(), Value::String(m));
                }
            }

            // Default: title + text if nothing specific requested
            if !title && !links && !text && !markdown && selector.is_none() {
                data.insert("title".into(), Value::String(content.title.clone()));
                data.insert("text".into(), Value::String(content.markdown.clone()));
            }

            let mut data_val = Value::Object(data);
            if let Some(mb) = max_bytes {
                crate::output::truncate_fields(&mut data_val, mb);
            }

            Ok((data_val, Some(tab_id)))
        }

        // ---- Content ----
        SessionCommand::Content { tab_id, format, max_bytes } => {
            let tab = get_tab(manager, &tab_id)?;
            let content = tab.content().await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;

            let mut data = match format.as_str() {
                "html" => serde_json::json!({
                    "url": content.url,
                    "title": content.title,
                    "status": content.status,
                    "html": content.html,
                }),
                "text" => serde_json::json!({
                    "url": content.url,
                    "title": content.title,
                    "status": content.status,
                    "text": content.markdown,
                }),
                _ => serde_json::json!({
                    "url": content.url,
                    "title": content.title,
                    "status": content.status,
                    "markdown": content.markdown,
                }),
            };

            if let Some(mb) = max_bytes {
                crate::output::truncate_fields(&mut data, mb);
            }

            Ok((data, Some(tab_id)))
        }

        // ---- Screenshot ----
        SessionCommand::Screenshot { tab_id, output_path, width } => {
            let tab = get_tab(manager, &tab_id)?;
            let w = width.unwrap_or(800);
            let png = tab.screenshot(w).await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;

            match output_path {
                Some(path) => {
                    std::fs::write(&path, &png).map_err(|e| {
                        CliResponse::error(format!("write failed: {e}"), "IO_ERROR")
                    })?;
                    Ok((serde_json::json!({
                        "saved": path,
                        "size": png.len(),
                        "width": w,
                    }), Some(tab_id)))
                }
                None => {
                    // Return base64-encoded PNG
                    use std::io::Write;
                    let mut buf = Vec::new();
                    {
                        let mut encoder = base64::write::EncoderWriter::new(&mut buf, &base64::engine::general_purpose::STANDARD);
                        encoder.write_all(&png).map_err(|e| {
                            CliResponse::error(format!("base64 encode failed: {e}"), "INTERNAL")
                        })?;
                    }
                    let b64 = String::from_utf8(buf).map_err(|e| {
                        CliResponse::error(format!("base64 encode failed: {e}"), "INTERNAL")
                    })?;
                    Ok((serde_json::json!({
                        "screenshot": b64,
                        "size": png.len(),
                        "width": w,
                        "encoding": "base64",
                    }), Some(tab_id)))
                }
            }
        }

        // ---- Wait ----
        SessionCommand::Wait { tab_id, selector, timeout_ms } => {
            let tab = get_tab(manager, &tab_id)?;
            let timeout = timeout_ms.unwrap_or(5000);
            tab.wait_for(&selector, timeout).await.map_err(|e| {
                CliResponse::error(format!("{e}"), crate::output::core_error_code(&e))
            })?;
            Ok((serde_json::json!({
                "waited": selector,
                "timeout_ms": timeout,
            }), Some(tab_id)))
        }

        // ---- Help ----
        SessionCommand::Help => {
            Ok((serde_json::json!({
                "commands": [
                    "new",
                    "goto <tab_id> <url> [--wait <selector>] [--timeout <ms>]",
                    "back <tab_id>",
                    "forward <tab_id>",
                    "reload <tab_id>",
                    "click <tab_id> <selector>",
                    "fill <tab_id> <selector> <value>",
                    "press <tab_id> <key>",
                    "type <tab_id> <selector> <text>",
                    "select <tab_id> <selector> <value>",
                    "check <tab_id> <selector>",
                    "uncheck <tab_id> <selector>",
                    "scroll <tab_id> <dx> <dy>",
                    "eval <tab_id> <expression> [--await]",
                    "extract <tab_id> [--selector <s>] [--all] [--attrs a,b] [--links] [--title] [--text] [--markdown] [--max-bytes N]",
                    "content <tab_id> [--format markdown|html|text] [--max-bytes N]",
                    "screenshot <tab_id> [-o path] [--width N]",
                    "wait <tab_id> <selector> [--timeout <ms>]",
                    "close <tab_id>",
                    "close --all",
                    "list",
                    "help",
                    "exit",
                ]
            }), None))
        }

        // ---- Exit ----
        SessionCommand::Exit => {
            // Not reached in normal flow (handled by caller), but provide a response anyway.
            Ok((serde_json::json!({ "exit": true }), None))
        }
    }
}

/// Get a tab from the manager, returning an error response if not found.
fn get_tab(manager: &TabManager, tab_id: &str) -> Result<oxibrowser_core::Tab, CliResponse> {
    manager.get(tab_id).cloned().ok_or_else(|| {
        CliResponse::error(
            format!("tab not found: {tab_id}"),
            "TAB_NOT_FOUND",
        )
    })
}
