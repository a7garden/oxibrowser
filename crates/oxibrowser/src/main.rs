//! OxiBrowser CLI — headless browser with CDP support.
//!
//! Subcommands:
//! - `oxibrowser fetch <url>` — fetch a URL and dump HTML/markdown (enhanced)
//! - `oxibrowser eval <url> <expr>` — evaluate JS on a page
//! - `oxibrowser extract <url>` — extract structured data from a page
//! - `oxibrowser browse <url>` — interactive Tab API browsing (CDP-free)
//! - `oxibrowser serve` — start the CDP server
//! - `oxibrowser version` — print version

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

/// OxiBrowser — a headless browser engine with CDP support.
#[derive(Parser)]
#[command(name = "oxibrowser")]
#[command(version, about = "Headless browser engine with CDP support")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch a URL and dump its content.
    Fetch {
        /// URL to fetch.
        url: String,
        /// Output format: html, markdown, or text.
        #[arg(long, default_value = "html")]
        format: String,
        /// Print response headers to stderr.
        #[arg(long)]
        headers: bool,
        /// Print only the HTTP status code.
        #[arg(long)]
        status: bool,
        /// HTTP method (GET or POST).
        #[arg(long, default_value = "GET")]
        method: String,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
        /// Timeout in seconds.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },

    /// Evaluate JavaScript on a page.
    Eval {
        /// URL to navigate to.
        url: String,
        /// JavaScript expression to evaluate.
        expression: String,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
        /// Timeout in seconds.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },

    /// Extract structured data from a page.
    Extract {
        /// URL to extract from.
        url: String,
        /// Extract all <a href> links (one per line).
        #[arg(long)]
        links: bool,
        /// Extract the <title> text.
        #[arg(long)]
        title: bool,
        /// Extract full text content.
        #[arg(long)]
        text: bool,
        /// Convert page to Markdown.
        #[arg(long)]
        markdown: bool,
        /// CSS selector to match elements.
        #[arg(long)]
        selector: Option<String>,
        /// With --selector, print all matching elements (not just the first).
        #[arg(long)]
        all: bool,
        /// Output as JSON.
        #[arg(long)]
        json: bool,
        /// Timeout in seconds.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },

    /// Browse a page using the Tab API (CDP-free).
    Browse {
        /// URL to navigate to.
        url: String,
        /// Output format: markdown, html, text, json, links.
        #[arg(long, default_value = "markdown")]
        format: String,
        /// Click an element matching a CSS selector.
        #[arg(long)]
        click: Option<String>,
        /// Fill an element (format: "selector:text").
        #[arg(long)]
        input: Option<String>,
        /// Press a key combo (e.g., Enter, Ctrl+C, Shift+Tab).
        #[arg(long)]
        press: Option<String>,
        /// Wait for a CSS selector before continuing.
        #[arg(long)]
        wait: Option<String>,
        /// Wait timeout in ms.
        #[arg(long, default_value_t = 5000)]
        wait_timeout: u64,
        /// Extract text from elements matching a selector.
        #[arg(long)]
        extract: Option<String>,
        /// With --extract, print all matches.
        #[arg(long)]
        all: bool,
        /// Save PNG screenshot to a file.
        #[arg(long)]
        screenshot: Option<String>,
        /// Screenshot width in pixels.
        #[arg(long, default_value_t = 800)]
        width: u32,
        /// Evaluate JS and print result.
        #[arg(long)]
        eval: Option<String>,
        /// Print response metadata to stderr.
        #[arg(long)]
        headers: bool,
        /// Timeout in seconds.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },

    /// Start the CDP server.
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
        /// Request timeout in seconds.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },

    /// Run a YAML script on a Tab.
    Run {
        /// Path to the YAML script file or inline YAML.
        script: String,
        /// Timeout in seconds.
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },

    /// Print version information.
    Version,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // Set up Ctrl+C handler for graceful shutdown.
    let ctrlc = tokio::signal::ctrl_c();
    tokio::pin!(ctrlc);

    match cli.command {
        Commands::Fetch {
            url,
            format,
            headers,
            status,
            method,
            json,
            timeout,
        } => {
            let op = run_fetch(&url, &format, headers, status, &method, json, timeout);
            tokio::select! {
                result = op => result?,
                _ = &mut ctrlc => {
                    eprintln!("\nInterrupted.");
                    std::process::exit(130);
                }
            }
        }
        Commands::Eval {
            url,
            expression,
            json,
            timeout,
        } => {
            let op = run_eval(&url, &expression, json, timeout);
            tokio::select! {
                result = op => result?,
                _ = &mut ctrlc => {
                    eprintln!("\nInterrupted.");
                    std::process::exit(130);
                }
            }
        }
        Commands::Extract {
            url,
            links,
            title,
            text,
            markdown,
            selector,
            all,
            json,
            timeout,
        } => {
            let op = run_extract(
                &url,
                links,
                title,
                text,
                markdown,
                selector.as_deref(),
                all,
                json,
                timeout,
            );
            tokio::select! {
                result = op => result?,
                _ = &mut ctrlc => {
                    eprintln!("\nInterrupted.");
                    std::process::exit(130);
                }
            }
        }
        Commands::Browse {
            url,
            format,
            click,
            input,
            press,
            wait,
            wait_timeout,
            extract,
            all,
            screenshot,
            width,
            eval,
            headers,
            timeout,
        } => {
            let op = run_browse(
                &url,
                &format,
                click.as_deref(),
                input.as_deref(),
                press.as_deref(),
                wait.as_deref(),
                wait_timeout,
                extract.as_deref(),
                all,
                screenshot.as_deref(),
                width,
                eval.as_deref(),
                headers,
                timeout,
            );
            tokio::select! {
                result = op => result?,
                _ = &mut ctrlc => {
                    eprintln!("\nInterrupted.");
                    std::process::exit(130);
                }
            }
        }
        Commands::Serve {
            host,
            port,
            cookie_file,
            timeout: _timeout,
        } => {
            let op = run_serve(&host, port, cookie_file.as_deref());
            tokio::select! {
                result = op => result?,
                _ = &mut ctrlc => {
                    // Ctrl+C is handled inside run_serve via its own ctrl_c listener.
                    // This branch is a fallback.
                }
            }
        }
        Commands::Run { script, timeout } => {
            let op = run_script(&script, timeout);
            tokio::select! {
                result = op => result?,
                _ = &mut ctrlc => {
                    eprintln!("\nInterrupted.");
                    std::process::exit(130);
                }
            }
        }
        Commands::Version => {
            println!("oxibrowser {}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// fetch
// ---------------------------------------------------------------------------

/// Fetch a URL and print the content.
async fn run_fetch(
    url: &str,
    format: &str,
    show_headers: bool,
    status_only: bool,
    method: &str,
    json_output: bool,
    timeout: u64,
) -> Result<()> {
    let _ = method; // Method selection is a future enhancement (HTTP client currently GETs only).

    info!(url = %url, format = %format, timeout, "fetching URL");

    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = oxibrowser_core::Browser::new(config).await?;

    let session_result =
        tokio::time::timeout(Duration::from_secs(timeout), browser.new_page(url)).await;

    let session = match session_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("error: timed out after {timeout}s");
            std::process::exit(1);
        }
    };

    let session_guard = session.read().await;

    match session_guard.page() {
        Some(page) => {
            if status_only {
                if json_output {
                    println!("{}", serde_json::json!({"status": page.status()}));
                } else {
                    println!("{}", page.status());
                }
            } else {
                if show_headers {
                    if json_output {
                        eprintln!(
                            "{}",
                            serde_json::json!({
                                "status": page.status(),
                                "content_type": page.content_type(),
                            })
                        );
                    } else {
                        eprintln!("HTTP {}", page.status());
                        eprintln!("Content-Type: {}", page.content_type());
                    }
                }

                let body = match format {
                    "markdown" | "md" => page.to_markdown(),
                    "text" => page
                        .root_frame()
                        .document()
                        .query_text("body")
                        .unwrap_or_default(),
                    _ => page.content().to_string(),
                };

                if json_output {
                    println!(
                        "{}",
                        serde_json::json!({
                            "status": page.status(),
                            "content_type": page.content_type(),
                            "format": format,
                            "body": body,
                        })
                    );
                } else {
                    print!("{body}");
                }
            }
        }
        None => {
            eprintln!("error: no page loaded");
            std::process::exit(1);
        }
    }

    drop(session_guard);
    browser.close().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// eval
// ---------------------------------------------------------------------------

/// Evaluate JavaScript on a page and print the result.
async fn run_eval(url: &str, expression: &str, json_output: bool, timeout: u64) -> Result<()> {
    info!(url = %url, expr = %expression, timeout, "evaluating JS");

    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = oxibrowser_core::Browser::new(config).await?;

    let session_result =
        tokio::time::timeout(Duration::from_secs(timeout), browser.new_page(url)).await;

    let session = match session_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("error: timed out after {timeout}s");
            std::process::exit(1);
        }
    };

    // Need write access for evaluate_js (it takes &mut Session).
    let result = {
        let mut guard = session.write().await;
        guard.evaluate_js(expression).await?
    };

    if json_output {
        // JSON output: always produce valid JSON.
        if result.is_ok() {
            let value = result.value.unwrap_or(serde_json::Value::Null);
            println!("{value}");
        } else {
            let err = result.exception.as_deref().unwrap_or("unknown error");
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    } else if result.is_ok() {
        if let Some(v) = &result.value {
            match v {
                serde_json::Value::String(s) => println!("{s}"),
                serde_json::Value::Null => {}
                other => println!("{other}"),
            }
        }
        // void/undefined: nothing to print
        // Also print any captured console output.
        for line in &result.console_output {
            eprintln!("[console] {line}");
        }
    } else {
        let err = result.exception.as_deref().unwrap_or("unknown error");
        eprintln!("error: {err}");
        std::process::exit(1);
    }

    browser.close().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

/// Extract structured data from a page.
#[allow(clippy::too_many_arguments)]
async fn run_extract(
    url: &str,
    links: bool,
    title: bool,
    text: bool,
    markdown: bool,
    selector: Option<&str>,
    all: bool,
    json_output: bool,
    timeout: u64,
) -> Result<()> {
    info!(url = %url, timeout, "extracting data");

    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = oxibrowser_core::Browser::new(config).await?;

    let session_result =
        tokio::time::timeout(Duration::from_secs(timeout), browser.new_page(url)).await;

    let session = match session_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("error: timed out after {timeout}s");
            std::process::exit(1);
        }
    };

    let session_guard = session.read().await;

    let page = match session_guard.page() {
        Some(p) => p,
        None => {
            eprintln!("error: no page loaded");
            std::process::exit(1);
        }
    };

    let frame = page.root_frame();
    let doc = frame.document();

    // Collect results into a JSON object for --json mode, otherwise print directly.
    let mut json_map = serde_json::Map::new();

    if title {
        let title_text = page.title().unwrap_or("").to_string();
        if json_output {
            json_map.insert("title".into(), serde_json::Value::String(title_text));
        } else {
            println!("{title_text}");
        }
    }

    if links {
        let link_nodes = doc.query_selector_all("a");
        let hrefs: Vec<String> = link_nodes
            .iter()
            .filter_map(|id| {
                doc.get_node(*id)
                    .and_then(|n| n.href().map(|h| h.to_string()))
            })
            .collect();
        if json_output {
            json_map.insert(
                "links".into(),
                serde_json::Value::Array(
                    hrefs.into_iter().map(serde_json::Value::String).collect(),
                ),
            );
        } else {
            for href in &hrefs {
                println!("{href}");
            }
        }
    }

    if text {
        let body_text = doc.query_text("body").unwrap_or_default();
        if json_output {
            json_map.insert("text".into(), serde_json::Value::String(body_text));
        } else {
            print!("{body_text}");
        }
    }

    if markdown {
        let md = page.to_markdown();
        if json_output {
            json_map.insert("markdown".into(), serde_json::Value::String(md));
        } else {
            print!("{md}");
        }
    }

    if let Some(sel) = selector {
        if all {
            let node_ids = doc.query_selector_all(sel);
            let texts: Vec<String> = node_ids
                .iter()
                .filter_map(|id| doc.text_content(*id))
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
            if json_output {
                json_map.insert(
                    "selector".into(),
                    serde_json::Value::String(sel.to_string()),
                );
                json_map.insert(
                    "matches".into(),
                    serde_json::Value::Array(
                        texts.into_iter().map(serde_json::Value::String).collect(),
                    ),
                );
            } else {
                for t in &texts {
                    println!("{t}");
                }
            }
        } else {
            let text_val = doc
                .query_text(sel)
                .map(|t| t.trim().to_string())
                .unwrap_or_default();
            if json_output {
                json_map.insert(
                    "selector".into(),
                    serde_json::Value::String(sel.to_string()),
                );
                json_map.insert("match".into(), serde_json::Value::String(text_val));
            } else {
                println!("{text_val}");
            }
        }
    }

    // If no extract flags were given, default to dumping the page title + text.
    if !title && !links && !text && !markdown && selector.is_none() {
        let title_text = page.title().unwrap_or("").to_string();
        let body_text = doc.query_text("body").unwrap_or_default();
        if json_output {
            json_map.insert("title".into(), serde_json::Value::String(title_text));
            json_map.insert("text".into(), serde_json::Value::String(body_text));
        } else {
            if !title_text.is_empty() {
                println!("Title: {title_text}");
            }
            println!("{body_text}");
        }
    }

    if json_output {
        println!("{}", serde_json::Value::Object(json_map));
    }

    drop(session_guard);
    browser.close().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// browse
// ---------------------------------------------------------------------------

/// Browse a page using the Tab API (CDP-free).
#[allow(clippy::too_many_arguments)]
async fn run_browse(
    url: &str,
    format: &str,
    click: Option<&str>,
    input: Option<&str>,
    press: Option<&str>,
    wait: Option<&str>,
    wait_timeout: u64,
    extract: Option<&str>,
    all: bool,
    screenshot: Option<&str>,
    width: u32,
    eval: Option<&str>,
    headers: bool,
    timeout: u64,
) -> Result<()> {
    info!(url = %url, timeout, "browsing URL");

    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = oxibrowser_core::Browser::new(config).await?;
    let tab = browser.new_tab().await?;

    let nav_result = tokio::time::timeout(Duration::from_secs(timeout), tab.goto(url)).await;
    let nav = match nav_result {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("error: timed out after {timeout}s");
            std::process::exit(1);
        }
    };

    if headers {
        eprintln!("HTTP {}", nav.status);
        eprintln!("URL: {}", nav.url);
        eprintln!("Title: {}", nav.title);
    }

    if let Some(selector) = wait {
        tab.wait_for(selector, wait_timeout).await?;
    }
    if let Some(selector) = click {
        tab.click(selector).await?;
    }
    if let Some(spec) = input {
        let (selector, value) = spec
            .split_once(':')
            .ok_or_else(|| anyhow!("--input must be in the form selector:text"))?;
        tab.fill(selector, value).await?;
    }
    if let Some(keys) = press {
        tab.press(keys).await?;
    }

    if let Some(path) = screenshot {
        let png = tab.screenshot(width).await?;
        std::fs::write(path, &png)?;
        eprintln!("Screenshot: {path} ({} bytes)", png.len());
    }

    if let Some(js) = eval {
        let value = tab.evaluate(js).await?;
        match value {
            serde_json::Value::String(s) => print!("{s}"),
            serde_json::Value::Null => {}
            other => print!("{other}"),
        }
    } else if let Some(selector) = extract {
        let matches = tab.query_all(selector).await?;
        if all {
            for text in matches {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    println!("{trimmed}");
                }
            }
        } else if let Some(first) = matches.iter().find(|t| !t.trim().is_empty()) {
            println!("{}", first.trim());
        }
    } else {
        let content = tab.content().await?;
        match format {
            "markdown" | "md" => print!("{}", content.markdown),
            "html" => print!("{}", content.html),
            "text" => {
                if let serde_json::Value::String(body) = tab
                    .evaluate("document.body ? document.body.textContent : ''")
                    .await?
                {
                    print!("{body}");
                }
            }
            "json" => println!(
                "{}",
                serde_json::to_string_pretty(&content).unwrap_or_default()
            ),
            "links" => {
                let value = tab
                    .evaluate("Array.from(document.querySelectorAll('a[href]')).map(a => a.href)")
                    .await?;
                if let serde_json::Value::Array(items) = value {
                    for item in items {
                        if let Some(link) = item.as_str() {
                            println!("{link}");
                        }
                    }
                }
            }
            _ => print!("{}", content.markdown),
        }
    }

    tab.close().await?;
    browser.close().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// serve
// ---------------------------------------------------------------------------

/// Start the CDP server with a real Browser instance.
async fn run_serve(host: &str, port: u16, cookie_file: Option<&str>) -> Result<()> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e: std::net::AddrParseError| anyhow::anyhow!("invalid address: {e}"))?;

    info!(addr = %addr, "starting CDP server");

    let mut config = oxibrowser_core::BrowserConfig::headless();
    if let Some(path) = cookie_file {
        config.cookie_file = Some(std::path::PathBuf::from(path));
    }
    // Disable SSRF filter for CDP server mode — clients navigate to arbitrary URLs
    config.enable_ssrf_filter = false;
    let browser = Arc::new(oxibrowser_core::Browser::new(config).await?);

    let server = Arc::new(oxibrowser_cdp::CdpServer::new(addr, browser.clone()));
    let bound_addr = server.start().await?;

    info!(addr = %bound_addr, "CDP server ready");
    println!("OxiBrowser CDP server listening on {bound_addr}");
    println!("  DevTools: http://{bound_addr}/json/version");
    println!("  WebSocket: ws://{bound_addr}/ws");

    tokio::signal::ctrl_c().await?;
    info!("shutting down");

    server.shutdown();
    browser.close().await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// run script
// ---------------------------------------------------------------------------

use oxibrowser_core::script::{parse_script, ScriptRunner};

/// Run a YAML script on a new Tab.
async fn run_script(script_path_or_yaml: &str, timeout: u64) -> Result<()> {
    info!(script = %script_path_or_yaml, timeout, "running script");

    let script_config = if std::path::Path::new(script_path_or_yaml).exists() {
        parse_script(&std::fs::read_to_string(script_path_or_yaml)?)
            .map_err(|e| anyhow::anyhow!("failed to parse script: {e}"))?
    } else {
        parse_script(script_path_or_yaml)
            .map_err(|e| anyhow::anyhow!("failed to parse script: {e}"))?
    };

    // Create a browser and a tab for the script
    let mut browser_config = oxibrowser_core::BrowserConfig::headless();
    browser_config.enable_ssrf_filter = false; // Allow script to navigate anywhere
    let browser = oxibrowser_core::Browser::new(browser_config).await?;

    // Create a tab for the script
    let tab = browser
        .new_tab()
        .await
        .map_err(|e| anyhow::anyhow!("failed to create tab: {e}"))?;

    // Run the script
    let mut runner = ScriptRunner::new(&tab);
    let result = runner
        .run_config(&script_config)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Print the result
    println!("{}", serde_json::to_string_pretty(&result).unwrap());

    if result.success {
        info!("script completed successfully");
    } else {
        eprintln!("script completed with errors");
    }

    browser.close().await?;
    Ok(())
}
