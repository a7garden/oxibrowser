//! `oxibrowser search` — integrated web search command.
//!
//! Router: dispatches to engines based on `--source` and `--engine` flags.
//! Web engines run in parallel, results are merged with URL-based dedup.

pub mod bing;
pub mod ddg;
pub mod engine;
pub mod github;
pub mod wiki;

use std::collections::HashSet;
use std::time::Duration;

use engine::{build_search_client, SearchEngine, SearchError, SearchOutput, SearchResult};

// ---------------------------------------------------------------------------
// WebEngine enum — concrete dispatch without trait objects
// ---------------------------------------------------------------------------

/// Supported web search engines.
enum WebEngine {
    DuckDuckGo(ddg::DuckDuckGoEngine),
    Wikipedia(wiki::WikipediaEngine),
    Bing(bing::BingEngine),
}

impl WebEngine {
    fn name(&self) -> &'static str {
        match self {
            Self::DuckDuckGo(e) => e.name(),
            Self::Wikipedia(e) => e.name(),
            Self::Bing(e) => e.name(),
        }
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>, SearchError> {
        match self {
            Self::DuckDuckGo(e) => e.search(query, max_results).await,
            Self::Wikipedia(e) => e.search(query, max_results).await,
            Self::Bing(e) => e.search(query, max_results).await,
        }
    }
}

const ENGINE_NAMES: &[&str] = &["ddg", "wiki", "bing"];

/// Parse `--engine` spec (comma-separated) into WebEngine instances.
fn parse_engines(spec: &str, client: reqwest::Client) -> Result<Vec<WebEngine>, SearchError> {
    let mut engines = Vec::new();
    for name in spec.split(',') {
        match name.trim() {
            "ddg" => engines.push(WebEngine::DuckDuckGo(ddg::DuckDuckGoEngine::new(client.clone()))),
            "wiki" => engines.push(WebEngine::Wikipedia(wiki::WikipediaEngine::new(client.clone()))),
            "bing" => engines.push(WebEngine::Bing(bing::BingEngine::new(client.clone()))),
            other => {
                return Err(SearchError::Parse(format!(
                    "unknown engine '{other}'. Valid engines: {}",
                    ENGINE_NAMES.join(", ")
                )));
            }
        }
    }
    if engines.is_empty() {
        return Err(SearchError::Parse("no engines specified in --engine".into()));
    }
    Ok(engines)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Execute a search and return structured output.
///
/// # Arguments
/// * `query`        — search query
/// * `source`       — "web", "github", or "github-issues"
/// * `engine_spec`  — comma-separated engines for web source (e.g. "ddg,wiki,bing")
/// * `repo`         — "owner/repo" for github-issues
/// * `token`        — optional GitHub PAT
/// * `max_results`  — 1..30
/// * `timeout_secs` — per-engine timeout
pub async fn dispatch(
    query: &str,
    source: &str,
    engine_spec: &str,
    repo: Option<&str>,
    token: Option<&str>,
    max_results: usize,
    timeout_secs: u64,
) -> Result<SearchOutput, SearchError> {
    let max_results = max_results.max(1).min(30);
    let client = build_search_client(timeout_secs);

    match source {
        "web" => search_web(query, engine_spec, client, max_results, timeout_secs).await,
        "github" => search_github_repos(query, client, token, max_results).await,
        "github-issues" => search_github_issues(query, repo, client, token, max_results).await,
        other => Err(SearchError::Parse(format!(
            "unknown source '{other}'. Valid: web, github, github-issues"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Web search (multi-engine, parallel)
// ---------------------------------------------------------------------------

async fn search_web(
    query: &str,
    engine_spec: &str,
    client: reqwest::Client,
    max_results: usize,
    timeout_secs: u64,
) -> Result<SearchOutput, SearchError> {
    let engines = parse_engines(engine_spec, client)?;
    let engine_label = engines.iter().map(|e| e.name()).collect::<Vec<_>>().join(",");
    let timeout = Duration::from_secs(timeout_secs);

    // Run all engines in parallel with per-engine timeout
    let futs: Vec<_> = engines
        .iter()
        .map(|engine| tokio::time::timeout(timeout, engine.search(query, max_results)))
        .collect();

    let completed = futures::future::join_all(futs).await;

    // Merge results: engine order → dedup by URL
    let mut all_results: Vec<SearchResult> = Vec::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut any_success = false;

    for (i, result) in completed.into_iter().enumerate() {
        match result {
            Ok(Ok(items)) => {
                any_success = true;
                for item in items {
                    if seen_urls.insert(item.url.clone()) {
                        all_results.push(item);
                    }
                }
            }
            Ok(Err(e)) => tracing::warn!("engine[{i}] search failed: {e}"),
            Err(_) => tracing::warn!("engine[{i}] timed out"),
        }
    }

    if !any_success && all_results.is_empty() {
        return Err(SearchError::Network("all search engines failed".into()));
    }

    all_results.truncate(max_results);

    Ok(SearchOutput {
        query: query.to_string(),
        source: "web".into(),
        engine: engine_label,
        total_results: all_results.len(),
        results: all_results,
    })
}

// ---------------------------------------------------------------------------
// GitHub searches
// ---------------------------------------------------------------------------

async fn search_github_repos(
    query: &str,
    client: reqwest::Client,
    token: Option<&str>,
    max_results: usize,
) -> Result<SearchOutput, SearchError> {
    let engine = github::GitHubEngine::repos(client, token.map(|s| s.to_string()));
    let results = engine.search(query, max_results).await?;

    Ok(SearchOutput {
        query: query.to_string(),
        source: "github".into(),
        engine: "github".into(),
        total_results: results.len(),
        results,
    })
}

async fn search_github_issues(
    query: &str,
    repo: Option<&str>,
    client: reqwest::Client,
    token: Option<&str>,
    max_results: usize,
) -> Result<SearchOutput, SearchError> {
    let repo = repo
        .ok_or_else(|| SearchError::Parse("--repo is required for source 'github-issues'".into()))?;
    let engine = github::GitHubEngine::issues(client, token.map(|s| s.to_string()), repo.to_string());
    let results = engine.search(query, max_results).await?;

    Ok(SearchOutput {
        query: query.to_string(),
        source: "github-issues".into(),
        engine: "github-issues".into(),
        total_results: results.len(),
        results,
    })
}

// ---------------------------------------------------------------------------
// Human output formatting
// ---------------------------------------------------------------------------

/// Format search results for terminal display.
pub fn format_human(output: &SearchOutput) {
    if output.results.is_empty() {
        println!("No results found.");
        return;
    }

    for (i, item) in output.results.iter().enumerate() {
        println!("{}. {}", i + 1, item.title);
        println!("   {}", item.url);
        if !item.snippet.is_empty() {
            println!("   {}", item.snippet);
        }
        print!("   [{}", item.source);
        if let Some(extra) = &item.extra {
            print!("] ⭐{} 🍴{}", extra.stars, extra.forks);
            if let Some(ref lang) = extra.language {
                print!(" 🔧{lang}");
            }
        }
        println!("]");
        println!();
    }

    eprintln!("{} result(s) | source: {} | engine: {}",
        output.total_results, output.source, output.engine);
}
