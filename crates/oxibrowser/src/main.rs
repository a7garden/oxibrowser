//! OxiBrowser CLI — headless browser with CDP support.
//!
//! Subcommands:
//! - `oxibrowser fetch <url>` — fetch a URL and dump HTML/markdown
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
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Fetch { url, format } => {
            run_fetch(&url, &format).await?;
        }
        Commands::Serve { host, port } => {
            run_serve(&host, port).await?;
        }
        Commands::Version => {
            println!("oxibrowser {}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}

/// Fetch a URL and print the content.
async fn run_fetch(url: &str, format: &str) -> Result<()> {
    info!(url = %url, format = %format, "fetching URL");

    let config = oxibrowser_core::BrowserConfig::headless();
    let browser = oxibrowser_core::Browser::new(config).await?;
    let session = browser.new_page(url).await?;
    let session_guard = session.read().await;

    match session_guard.page() {
        Some(page) => match format {
            "markdown" | "md" => {
                println!("{}", page.to_markdown());
            }
            "text" => {
                // Get text content from the root frame
                if let Some(text) = page.root_frame().document().query_text("body") {
                    println!("{}", text);
                }
            }
            _ => {
                // Default: HTML
                println!("{}", page.content());
            }
        },
        None => {
            eprintln!("No page loaded");
        }
    }

    drop(session_guard);
    browser.close().await?;
    Ok(())
}

/// Start the CDP server.
async fn run_serve(host: &str, port: u16) -> Result<()> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e: std::net::AddrParseError| anyhow::anyhow!("invalid address: {e}"))?;

    info!(addr = %addr, "starting CDP server");

    let server = Arc::new(oxibrowser_cdp::CdpServer::new(addr));
    let bound_addr = server.start().await?;

    info!(addr = %bound_addr, "CDP server ready");
    println!("OxiBrowser CDP server listening on {bound_addr}");
    println!("  DevTools: http://{bound_addr}/json/version");
    println!("  WebSocket: ws://{bound_addr}/ws");

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    info!("shutting down");
    server.shutdown();

    Ok(())
}
