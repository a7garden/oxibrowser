//! OxiBrowser CLI — headless browser with CDP support.
//!
//! Subcommands:
//! - `oxibrowser fetch <url>` — fetch a URL and dump HTML/markdown (enhanced)
//! - `oxibrowser eval <url> <expr>` — evaluate JS on a page
//! - `oxibrowser extract <url>` — extract structured data from a page
//! - `oxibrowser serve` — start the CDP server
//! - `oxibrowser version` — print version

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use std::sync::Arc;
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
    },

    /// Start the CDP server.
    Serve {
        /// Host to bind to.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to listen on.
        #[arg(long, default_value_t = 9222)]
        port: u16,
    },

    /// Print version information.
    Version,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Fetch {
            url,
            format,
            headers,
            status,
            method,
            json,
        } => run_fetch(&url, &format, headers, status, &method, json).await?,
        Commands::Eval {
            url,
            expression,
            json,
        } => run_eval(&url, &expression, json).await?,
        Commands::Extract {
            url,
            links,
            title,
            text,
            markdown,
            selector,
            all,
            json,
        } => run_extract(&url, links, title, text, markdown, selector.as_deref(), all, json)
            .await?,
        Commands::Serve { host, port } => run_serve(&host, port).await?,
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
) -> Result<()> {
    let _ = method; // Method selection is a future enhancement (HTTP client currently GETs only).

    info!(url = %url, format = %format, "fetching URL");

    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = oxibrowser_core::Browser::new(config).await?;
    let session = browser.new_page(url).await?;
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
async fn run_eval(url: &str, expression: &str, json_output: bool) -> Result<()> {
    info!(url = %url, expr = %expression, "evaluating JS");

    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = oxibrowser_core::Browser::new(config).await?;
    let session = browser.new_page(url).await?;

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
) -> Result<()> {
    info!(url = %url, "extracting data");

    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = oxibrowser_core::Browser::new(config).await?;
    let session = browser.new_page(url).await?;
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
            .filter_map(|id| doc.get_node(*id).and_then(|n| n.href().map(|h| h.to_string())))
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
            json_map.insert(
                "text".into(),
                serde_json::Value::String(body_text),
            );
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
// serve
// ---------------------------------------------------------------------------

/// Start the CDP server with a real Browser instance.
async fn run_serve(host: &str, port: u16) -> Result<()> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e: std::net::AddrParseError| anyhow::anyhow!("invalid address: {e}"))?;

    info!(addr = %addr, "starting CDP server");

    let config = oxibrowser_core::BrowserConfig::headless();
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
