//! OxiBrowser CLI 2.0 — headless browser for AI agents.
//!
//! Human is the default. `--json` opts into machine-readable output.
//!
//! 7 subcommands: fetch, extract, run, session, serve, describe, skill, version

use clap::{Parser, Subcommand};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

mod describe;
mod output;
mod session;
mod skill;
mod validate;

/// OxiBrowser — headless browser for AI agents.
#[derive(Parser)]
#[command(name = "oxibrowser")]
#[command(version, about = "Headless browser for AI agents — single static binary, no Chromium")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch a URL and return content. Supports interaction before output.
    Fetch {
        /// URL to fetch.
        url: String,
        /// Output format: html, markdown, or text.
        #[arg(long, default_value = "markdown")]
        format: String,
        /// Output as JSON (for agents / scripting).
        #[arg(long)]
        json: bool,
        /// Truncate output at N bytes (JSON mode).
        #[arg(long)]
        max_bytes: Option<u64>,
        /// Comma-separated fields to include: url,title,status,markdown,html,text,content_type.
        #[arg(long)]
        fields: Option<String>,
        /// Page metadata only (headings, links_count, text_length).
        #[arg(long)]
        summary: bool,
        /// Evaluate JS expression after page load.
        #[arg(long)]
        eval: Option<String>,
        /// Click element matching CSS selector.
        #[arg(long)]
        click: Option<String>,
        /// Fill input (format: "selector:value").
        #[arg(long)]
        fill: Option<String>,
        /// Press key (Enter, Tab, Ctrl+C, etc.).
        #[arg(long)]
        press: Option<String>,
        /// Wait for CSS selector before output.
        #[arg(long)]
        wait: Option<String>,
        /// Wait timeout in ms.
        #[arg(long, default_value_t = 5000)]
        wait_timeout: u64,
        /// Extract text from selector instead of full content.
        #[arg(long)]
        extract: Option<String>,
        /// With --extract: return all matches.
        #[arg(long)]
        all: bool,
        /// Print HTTP headers to stderr.
        #[arg(long)]
        headers: bool,
        /// Timeout in seconds.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },

    /// Extract structured data from a URL.
    Extract {
        /// URL to extract from.
        url: String,
        /// CSS selector to match elements.
        #[arg(long)]
        selector: Option<String>,
        /// Return all matches (not just first).
        #[arg(long)]
        all: bool,
        /// Comma-separated attributes to extract (text,href,data-*,src,...).
        #[arg(long, default_value = "text")]
        attrs: String,
        /// Extract all <a href> values.
        #[arg(long)]
        links: bool,
        /// Extract the <title> text.
        #[arg(long)]
        title: bool,
        /// Extract body text.
        #[arg(long)]
        text: bool,
        /// Extract page as markdown.
        #[arg(long)]
        markdown: bool,
        /// Truncate output at N bytes (JSON mode).
        #[arg(long)]
        max_bytes: Option<u64>,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
        /// Timeout in seconds.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },

    /// Run a YAML browser automation script.
    Run {
        /// Path to YAML script file or inline YAML.
        script: String,
        /// Timeout in seconds.
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },

    /// Start interactive session (stdin/stdout JSON REPL).
    Session,

    /// Start CDP server for Puppeteer/Playwright.
    Serve {
        /// Host to bind to.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to listen on.
        #[arg(long, default_value_t = 9222)]
        port: u16,
        /// Cookie persistence file.
        #[arg(long)]
        cookie_file: Option<String>,
    },

    /// Print CLI schema as JSON (for agents).
    Describe {
        /// Specific command to describe.
        command: Option<String>,
        /// Minimal output (~200 tokens).
        #[arg(long)]
        compact: bool,
    },

    /// Print agent skill guide.
    Skill,

    /// Print version information.
    Version,
}

// ---------------------------------------------------------------------------
// Output decision: --json → agent, otherwise → human.
// ---------------------------------------------------------------------------

/// Whether to use JSON output. Only true when --json is explicitly set.
fn use_json(explicit_json: bool) -> bool {
    explicit_json
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();

    let exit_code = match cli.command {
        Commands::Fetch {
            url, format, json, max_bytes, fields, summary, eval,
            click, fill, press, wait, wait_timeout, extract, all, headers, timeout,
        } => {
            run_fetch(
                &url, &format, json, max_bytes, fields.as_deref(), summary,
                eval.as_deref(), click.as_deref(), fill.as_deref(), press.as_deref(),
                wait.as_deref(), wait_timeout, extract.as_deref(), all, headers, timeout,
            ).await
        }
        Commands::Extract {
            url, selector, all, attrs, links, title, text, markdown,
            max_bytes, json, timeout,
        } => {
            run_extract(
                &url, selector.as_deref(), all, &attrs,
                links, title, text, markdown, max_bytes, json, timeout,
            ).await
        }
        Commands::Run { script, timeout } => run_script(&script, timeout).await,
        Commands::Session => session::run_session().await,
        Commands::Serve { host, port, cookie_file } => {
            run_serve(&host, port, cookie_file.as_deref()).await
        }
        Commands::Describe { command, compact } => run_describe(command.as_deref(), compact),
        Commands::Skill => { print!("{}", skill::skill_text()); 0 }
        Commands::Version => { println!("oxibrowser {}", env!("CARGO_PKG_VERSION")); 0 }
    };

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

// ---------------------------------------------------------------------------
// Error output — human vs JSON
// ---------------------------------------------------------------------------

/// Print an error and return the exit code.
fn print_error(msg: &str, error_code: &str, json: bool) -> i32 {
    let code = match error_code {
        "INVALID_URL" | "INVALID_SELECTOR" | "INPUT_VALIDATION" | "PATH_TRAVERSAL" | "SSRF_BLOCKED" => 2,
        "TIMEOUT" => 3,
        "NETWORK_ERROR" | "HTTP_ERROR" => 4,
        _ => 1,
    };

    if json {
        let resp = output::CliResponse::error(msg, error_code);
        resp.print_json();
    } else {
        eprintln!("Error: {msg}");
    }
    code
}

// ---------------------------------------------------------------------------
// fetch
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_fetch(
    url: &str, format: &str, json: bool, max_bytes: Option<u64>,
    fields: Option<&str>, summary: bool, eval: Option<&str>,
    click: Option<&str>, fill: Option<&str>, press: Option<&str>,
    wait: Option<&str>, wait_timeout: u64, extract_sel: Option<&str>,
    all: bool, headers: bool, timeout: u64,
) -> i32 {
    let start = Instant::now();
    let json = use_json(json);

    // Validate
    if let Some(e) = validate_fetch_inputs(url, click, fill, wait, extract_sel, eval) {
        return print_error(&e.error.unwrap_or_default(), &e.error_code.unwrap_or_default(), json);
    }

    let needs_tab = click.is_some() || fill.is_some() || press.is_some()
        || wait.is_some() || eval.is_some();

    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = match oxibrowser_core::Browser::new(config).await {
        Ok(b) => b,
        Err(e) => return print_error(&format!("browser init failed: {e}"), "RUNTIME_ERROR", json),
    };

    let result = if needs_tab {
        fetch_with_tab(
            start, &browser, url, format, json, max_bytes, fields, summary,
            eval, click, fill, press, wait, wait_timeout, extract_sel, all, headers, timeout,
        ).await
    } else {
        fetch_direct(
            start, &browser, url, format, json, max_bytes, fields, summary,
            extract_sel, all, headers,
        ).await
    };

    browser.close().await.ok();

    match result {
        Ok(()) => 0,
        Err(FetchError { msg, code }) => print_error(&msg, &code, json),
    }
}

struct FetchError {
    msg: String,
    code: String,
}

impl From<oxibrowser_core::error::CoreError> for FetchError {
    fn from(e: oxibrowser_core::error::CoreError) -> Self {
        FetchError {
            msg: format!("{e}"),
            code: output::core_error_code(&e).to_string(),
        }
    }
}

/// Direct fetch: no interaction needed.
#[allow(clippy::too_many_arguments)]
async fn fetch_direct(start: Instant, 
    browser: &oxibrowser_core::Browser,
    url: &str,
    format: &str,
    json: bool,
    max_bytes: Option<u64>,
    fields: Option<&str>,
    summary: bool,
    extract_sel: Option<&str>,
    all: bool,
    headers: bool,
) -> Result<(), FetchError> {
    let session = browser.new_page(url).await.map_err(FetchError::from)?;
    let guard = session.read().await;
    let page = guard.page().ok_or_else(|| FetchError {
        msg: "no page loaded".into(),
        code: "PAGE_NOT_LOADED".into(),
    })?;

    if headers {
        eprintln!("HTTP {}", page.status());
        eprintln!("Content-Type: {}", page.content_type());
    }

    // Summary — always JSON (structured metadata)
    if summary {
        let data = output::build_summary(page);
        if json {
            let resp = output::CliResponse::success_with_meta(data, None, start.elapsed().as_millis() as u64);
            resp.print_json();
        } else {
            // Human: print summary as key-value
            let obj = data.as_object().unwrap();
            if let Some(v) = obj.get("url").and_then(|v| v.as_str()) {
                eprintln!("URL: {v}");
            }
            if let Some(v) = obj.get("title").and_then(|v| v.as_str()) {
                eprintln!("Title: {v}");
            }
            eprintln!("Status: {}", obj.get("status").unwrap());
            if let Some(h) = obj.get("headings").and_then(|v| v.as_array()) {
                eprintln!("Headings: {}", h.len());
                for h in h {
                    if let Some(s) = h.as_str() { eprintln!("  - {s}"); }
                }
            }
            eprintln!("Links: {}", obj.get("links_count").unwrap());
            eprintln!("Forms: {}", obj.get("forms_count").unwrap());
            eprintln!("Images: {}", obj.get("images_count").unwrap());
            eprintln!("Text length: {}", obj.get("text_length").unwrap());
        }
        return Ok(());
    }

    // Extract — human gets text, agent gets JSON
    if let Some(sel) = extract_sel {
        let doc = page.root_frame().document();
        if all {
            let texts: Vec<String> = doc
                .query_selector_all(sel)
                .iter()
                .filter_map(|id| doc.text_content(*id))
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
            if json {
                let resp = output::CliResponse::success(serde_json::json!({
                    "selector": sel, "count": texts.len(), "items": texts
                }));
                resp.print_json();
            } else {
                for t in &texts { println!("{t}"); }
            }
        } else {
            let text = doc.query_text(sel).map(|t| t.trim().to_string()).unwrap_or_default();
            if json {
                let resp = output::CliResponse::success(serde_json::json!({
                    "selector": sel, "match": text
                }));
                resp.print_json();
            } else {
                println!("{text}");
            }
        }
        return Ok(());
    }

    // Full content
    let body = match format {
        "markdown" | "md" => page.to_markdown(),
        "text" => {
            // textContent has no line breaks (no CSS layout).
            // Use markdown → strip formatting for readable plain text.
            let md = page.to_markdown();
            // Strip markdown syntax: # headings, **bold**, [links](url), etc.
            let text = md
                .lines()
                .map(|line| {
                    let l = line.trim();
                    // Strip heading markers
                    let l = l.strip_prefix('#').map(|s| s.trim()).unwrap_or(l);
                    let l = l.strip_prefix('#').map(|s| s.trim()).unwrap_or(l);
                    // Strip bold/italic markers
                    let l = l.replace("**", "").replace("__", "");
                    let l = l.replace("* ", "");
                    // Convert [text](url) to just text
                    let l = regex_strip_link(&l);
                    l
                })
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            text
        }
        _ => page.content().to_string(),
    };

    if json {
        let mut data = serde_json::json!({
            "url": page.url().to_string(),
            "title": page.title().unwrap_or("").to_string(),
            "status": page.status(),
            "content_type": page.content_type().to_string(),
        });
        let key = match format {
            "markdown" | "md" => "markdown",
            "text" => "text",
            _ => "html",
        };
        data.as_object_mut().unwrap().insert(key.into(), Value::String(body));
        if let Some(mb) = max_bytes {
            output::truncate_fields(&mut data, mb);
        }
        if let Some(f) = fields {
            output::filter_fields(&mut data, &output::parse_fields(f));
        }
        output::CliResponse::success_with_meta(data, None, start.elapsed().as_millis() as u64).print_json();
    } else {
        print!("{body}");
    }
    Ok(())
}

/// Tab-based fetch: for interaction and JS eval.
#[allow(clippy::too_many_arguments)]
async fn fetch_with_tab(start: Instant, 
    browser: &oxibrowser_core::Browser,
    url: &str,
    format: &str,
    json: bool,
    max_bytes: Option<u64>,
    fields: Option<&str>,
    summary: bool,
    eval: Option<&str>,
    click: Option<&str>,
    fill: Option<&str>,
    press: Option<&str>,
    wait: Option<&str>,
    wait_timeout: u64,
    extract_sel: Option<&str>,
    all: bool,
    headers: bool,
    timeout: u64,
) -> Result<(), FetchError> {
    let tab = browser.new_tab().await.map_err(FetchError::from)?;

    let nav_result = tokio::time::timeout(Duration::from_secs(timeout), tab.goto(url)).await;
    match nav_result {
        Ok(Ok(nav)) => {
            if headers {
                eprintln!("HTTP {}", nav.status);
                eprintln!("URL: {}", nav.url);
                eprintln!("Title: {}", nav.title);
            }
        }
        Ok(Err(e)) => return Err(FetchError::from(e)),
        Err(_) => return Err(FetchError {
            msg: format!("timed out after {timeout}s"),
            code: "TIMEOUT".into(),
        }),
    }

    // Interaction: wait → fill → click → press
    if let Some(sel) = wait {
        tab.wait_for(sel, wait_timeout).await.map_err(FetchError::from)?;
    }
    if let Some(spec) = fill {
        let (sel, val) = spec.split_once(':').ok_or_else(|| FetchError {
            msg: "--fill must be selector:value".into(),
            code: "INPUT_VALIDATION".into(),
        })?;
        tab.fill(sel, val).await.map_err(FetchError::from)?;
    }
    if let Some(sel) = click {
        tab.click(sel).await.map_err(FetchError::from)?;
    }
    if let Some(keys) = press {
        tab.press(keys).await.map_err(FetchError::from)?;
    }

    // Eval
    if let Some(expr) = eval {
        let value = tab.evaluate(expr).await.map_err(FetchError::from)?;
        if json {
            output::CliResponse::success(serde_json::json!({"value": value})).print_json();
        } else {
            match value {
                Value::String(s) => println!("{s}"),
                Value::Null => {}
                other => println!("{other}"),
            }
        }
        return Ok(());
    }

    // Summary
    if summary {
        let content = tab.content().await.map_err(FetchError::from)?;
        let data = serde_json::json!({
            "url": content.url, "title": content.title,
            "status": content.status, "text_length": content.markdown.len(),
        });
        if json {
            output::CliResponse::success_with_meta(data, None, start.elapsed().as_millis() as u64).print_json();
        } else {
            eprintln!("URL: {}", content.url);
            eprintln!("Title: {}", content.title);
            eprintln!("Status: {}", content.status);
            eprintln!("Text length: {}", content.markdown.len());
        }
        return Ok(());
    }

    // Extract
    if let Some(sel) = extract_sel {
        let matches = tab.query_all(sel).await.map_err(FetchError::from)?;
        if all {
            let items: Vec<String> = matches.into_iter()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
            if json {
                output::CliResponse::success(serde_json::json!({
                    "selector": sel, "count": items.len(), "items": items
                })).print_json();
            } else {
                for t in &items { println!("{t}"); }
            }
        } else {
            let text = matches.first().map(|t| t.trim().to_string()).unwrap_or_default();
            if json {
                output::CliResponse::success(serde_json::json!({
                    "selector": sel, "match": text
                })).print_json();
            } else {
                println!("{text}");
            }
        }
        return Ok(());
    }

    // Full content
    let content = tab.content().await.map_err(FetchError::from)?;
    if json {
        let mut data = match format {
            "markdown" | "md" => serde_json::json!({
                "url": content.url, "title": content.title,
                "status": content.status, "markdown": content.markdown,
            }),
            "text" => {
                let body = content.markdown.split_whitespace().collect::<Vec<_>>().join(" ");
                serde_json::json!({
                    "url": content.url, "title": content.title,
                    "status": content.status, "text": body,
                })
            },
            _ => serde_json::json!({
                "url": content.url, "title": content.title,
                "status": content.status, "html": content.html,
            }),
        };
        if let Some(mb) = max_bytes {
            output::truncate_fields(&mut data, mb);
        }
        if let Some(f) = fields {
            output::filter_fields(&mut data, &output::parse_fields(f));
        }
        output::CliResponse::success_with_meta(data, None, start.elapsed().as_millis() as u64).print_json();
    } else {
        match format {
            "html" => print!("{}", content.html),
            "text" => {
                let body = content.markdown.lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                println!("{body}");
            }
            _ => print!("{}", content.markdown),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_extract(
    url: &str, selector: Option<&str>, all: bool, attrs: &str,
    links: bool, title: bool, text: bool, markdown: bool,
    max_bytes: Option<u64>, json: bool, timeout: u64,
) -> i32 {
    let json = use_json(json);
    let start = Instant::now();

    // Validate
    if let Err(e) = validate::validate_url(url) {
        return print_error(&e.to_string(), e.error_code(), json);
    }
    if let Some(sel) = selector {
        if let Err(e) = validate::validate_selector(sel) {
            return print_error(&e.to_string(), e.error_code(), json);
        }
    }

    let requested_attrs: Vec<&str> = output::parse_fields(attrs);

    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = match oxibrowser_core::Browser::new(config).await {
        Ok(b) => b,
        Err(e) => return print_error(&format!("browser init failed: {e}"), "RUNTIME_ERROR", json),
    };

    let session_result =
        tokio::time::timeout(Duration::from_secs(timeout), browser.new_page(url)).await;

    let session = match session_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            browser.close().await.ok();
            return print_error(&format!("{e}"), output::core_error_code(&e), json);
        }
        Err(_) => {
            browser.close().await.ok();
            return print_error(&format!("timed out after {timeout}s"), "TIMEOUT", json);
        }
    };

    let guard = session.read().await;
    let page = match guard.page() {
        Some(p) => p,
        None => {
            browser.close().await.ok();
            return print_error("no page loaded", "PAGE_NOT_LOADED", json);
        }
    };

    let doc = page.root_frame().document();
    let mut json_map = serde_json::Map::new();

    if title {
        json_map.insert("title".into(), Value::String(page.title().unwrap_or("").to_string()));
    }
    if links {
        let hrefs: Vec<Value> = doc.query_selector_all("a[href]")
            .iter()
            .filter_map(|id| doc.get_node(*id).and_then(|n| n.href().map(|h| Value::String(h.to_string()))))
            .collect();
        json_map.insert("links".into(), Value::Array(hrefs));
    }
    if text {
        json_map.insert("text".into(), Value::String(doc.query_text("body").unwrap_or_default()));
    }
    if markdown {
        json_map.insert("markdown".into(), Value::String(page.to_markdown()));
    }

    if let Some(sel) = selector {
        let ids = doc.query_selector_all(sel);
        if all {
            let items: Vec<Value> = ids.iter().filter_map(|id| {
                let mut item = serde_json::Map::new();
                for &attr in &requested_attrs {
                    let val = if attr == "text" {
                        doc.text_content(*id).map(|t| t.trim().to_string()).unwrap_or_default()
                    } else {
                        doc.get_node(*id).and_then(|n| n.get_attribute(attr).map(|v| v.to_string())).unwrap_or_default()
                    };
                    item.insert(attr.into(), Value::String(val));
                }
                if !item.is_empty() { Some(Value::Object(item)) } else { None }
            }).collect();
            json_map.insert("selector".into(), Value::String(sel.into()));
            json_map.insert("count".into(), Value::Number(serde_json::Number::from(items.len())));
            json_map.insert("items".into(), Value::Array(items));
        } else {
            let mut item = serde_json::Map::new();
            if let Some(id) = ids.first() {
                for &attr in &requested_attrs {
                    let val = if attr == "text" {
                        doc.text_content(*id).map(|t| t.trim().to_string()).unwrap_or_default()
                    } else {
                        doc.get_node(*id).and_then(|n| n.get_attribute(attr).map(|v| v.to_string())).unwrap_or_default()
                    };
                    item.insert(attr.into(), Value::String(val));
                }
            }
            json_map.insert("selector".into(), Value::String(sel.into()));
            json_map.insert("match".into(), Value::Object(item));
        }
    }

    // Default: title + text
    if !title && !links && !text && !markdown && selector.is_none() {
        json_map.insert("title".into(), Value::String(page.title().unwrap_or("").to_string()));
        json_map.insert("text".into(), Value::String(doc.query_text("body").unwrap_or_default()));
    }

    drop(guard);
    browser.close().await.ok();

    let mut data = Value::Object(json_map);
    if let Some(mb) = max_bytes {
        output::truncate_fields(&mut data, mb);
    }

    if json {
        output::CliResponse::success_with_meta(data, None, start.elapsed().as_millis() as u64).print_json();
    } else {
        print_extract_human(&data);
    }
    0
}

/// Print extract data in human-friendly format.
fn print_extract_human(data: &Value) {
    let obj = match data.as_object() {
        Some(o) => o,
        None => { println!("{data}"); return; }
    };

    // Title
    if let Some(title) = obj.get("title").and_then(|v| v.as_str()) {
        if !title.is_empty() { println!("Title: {title}"); }
    }
    // Blank line after title for visual separation
    if obj.contains_key("title") && (obj.contains_key("text") || obj.contains_key("items") || obj.contains_key("links")) {
        println!();
    }
    // Links: one per line
    if let Some(links) = obj.get("links").and_then(|v| v.as_array()) {
        for link in links {
            if let Some(s) = link.as_str() { println!("{s}"); }
        }
    }
    // Selector items
    if let Some(items) = obj.get("items").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(s) = item.as_str() {
                println!("{s}");
            } else {
                let vals: Vec<&str> = item.as_object()
                    .map(|o| o.values().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                println!("{}", vals.join("\t"));
            }
        }
    }
    // Single match
    if let Some(m) = obj.get("match") {
        if let Some(s) = m.as_str() {
            println!("{s}");
        } else {
            let vals: Vec<&str> = m.as_object()
                .map(|o| o.values().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            println!("{}", vals.join("\t"));
        }
    }
    // Body text
    if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            for line in text.lines() {
                println!("{line}");
            }
        }
    }
    // Markdown
    if let Some(md) = obj.get("markdown").and_then(|v| v.as_str()) {
        if !md.is_empty() { print!("{md}"); }
    }
}

// ---------------------------------------------------------------------------
// run (YAML script)
// ---------------------------------------------------------------------------

async fn run_script(script_path_or_yaml: &str, timeout: u64) -> i32 {
    let script_config = if std::path::Path::new(script_path_or_yaml).exists() {
        match std::fs::read_to_string(script_path_or_yaml) {
            Ok(content) => match oxibrowser_core::script::parse_script(&content) {
                Ok(cfg) => cfg,
                Err(e) => { eprintln!("Error: parse error: {e}"); return 1; }
            },
            Err(e) => { eprintln!("Error: cannot read script: {e}"); return 1; }
        }
    } else {
        match oxibrowser_core::script::parse_script(script_path_or_yaml) {
            Ok(cfg) => cfg,
            Err(e) => { eprintln!("Error: parse error: {e}"); return 1; }
        }
    };

    let mut browser_config = oxibrowser_core::BrowserConfig::headless();
    browser_config.enable_ssrf_filter = false;
    let browser = match oxibrowser_core::Browser::new(browser_config).await {
        Ok(b) => b,
        Err(e) => { eprintln!("Error: browser init failed: {e}"); return 1; }
    };

    let tab = match browser.new_tab().await {
        Ok(t) => t,
        Err(e) => { eprintln!("Error: tab creation failed: {e}"); return 1; }
    };

    let mut runner = oxibrowser_core::script::ScriptRunner::new(&tab);
    let script_result = match tokio::time::timeout(
        Duration::from_secs(timeout),
        runner.run_config(&script_config),
    ).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => { browser.close().await.ok(); eprintln!("Error: {e}"); return 1; }
        Err(_) => { browser.close().await.ok(); eprintln!("Error: timed out after {timeout}s"); return 3; }
    };

    browser.close().await.ok();
    println!("{}", serde_json::to_string_pretty(&script_result).unwrap());
    0
}

// ---------------------------------------------------------------------------
// session (Phase 2) → see session/mod.rs
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// serve (CDP server)
// ---------------------------------------------------------------------------

async fn run_serve(host: &str, port: u16, cookie_file: Option<&str>) -> i32 {
    let addr: SocketAddr = match format!("{host}:{port}").parse() {
        Ok(a) => a,
        Err(e) => { eprintln!("Error: invalid address: {e}"); return 2; }
    };

    info!(addr = %addr, "starting CDP server");

    let mut config = oxibrowser_core::BrowserConfig::headless();
    if let Some(path) = cookie_file {
        config.cookie_file = Some(std::path::PathBuf::from(path));
    }
    config.enable_ssrf_filter = false;

    let browser = match oxibrowser_core::Browser::new(config).await {
        Ok(b) => b,
        Err(e) => { eprintln!("Error: browser init failed: {e}"); return 1; }
    };
    let browser = Arc::new(browser);

    let server = Arc::new(oxibrowser_cdp::CdpServer::new(addr, browser.clone()));
    let bound_addr = match server.start().await {
        Ok(a) => a,
        Err(e) => { eprintln!("Error: server bind failed: {e}"); return 4; }
    };

    info!(addr = %bound_addr, "CDP server ready");
    println!("OxiBrowser CDP server listening on {bound_addr}");
    println!("  DevTools: http://{bound_addr}/json/version");
    println!("  WebSocket: ws://{bound_addr}/ws");

    tokio::signal::ctrl_c().await.ok();
    info!("shutting down");

    server.shutdown();
    browser.close().await.ok();
    0
}

// ---------------------------------------------------------------------------
// describe
// ---------------------------------------------------------------------------

fn run_describe(command: Option<&str>, compact: bool) -> i32 {
    // describe is agent-only — always JSON
    let response = match command {
        Some(cmd) => describe::describe_command(cmd),
        None => describe::describe_all(compact),
    };
    response.print_json()
}

// ---------------------------------------------------------------------------
// Text formatting helpers
// ---------------------------------------------------------------------------

/// Strip markdown link syntax: [text](url) → text
fn regex_strip_link(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut result = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            // Find matching ](
            if let Some(close) = bytes[i..].iter().position(|&b| b == b']') {
                let close_idx = i + close;
                if close_idx + 1 < bytes.len() && bytes[close_idx + 1] == b'(' {
                    // Find closing )
                    if let Some(paren) = bytes[close_idx + 2..].iter().position(|&b| b == b')') {
                        // Extract text between [ and ]
                        let text = &s[i + 1..close_idx];
                        result.push_str(text);
                        i = close_idx + 2 + paren + 1;
                        continue;
                    }
                }
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

// ---------------------------------------------------------------------------
// Validation helper
// ---------------------------------------------------------------------------

fn validate_fetch_inputs(
    url: &str, click: Option<&str>, fill: Option<&str>,
    wait: Option<&str>, extract: Option<&str>, eval: Option<&str>,
) -> Option<output::CliResponse> {
    if let Err(e) = validate::validate_url(url) {
        return Some(output::CliResponse::from_validation(e));
    }
    if let Some(sel) = click {
        if let Err(e) = validate::validate_selector(sel) {
            return Some(output::CliResponse::from_validation(e));
        }
    }
    if let Some(spec) = fill {
        if !spec.contains(':') {
            return Some(output::CliResponse::error(
                "--fill must be in the format selector:value",
                "INPUT_VALIDATION",
            ));
        }
    }
    if let Some(sel) = wait {
        if let Err(e) = validate::validate_selector(sel) {
            return Some(output::CliResponse::from_validation(e));
        }
    }
    if let Some(sel) = extract {
        if let Err(e) = validate::validate_selector(sel) {
            return Some(output::CliResponse::from_validation(e));
        }
    }
    if let Some(expr) = eval {
        if let Err(e) = validate::validate_expression(expr) {
            return Some(output::CliResponse::from_validation(e));
        }
    }
    None
}
