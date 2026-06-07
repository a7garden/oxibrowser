//! GitHub search engine — repositories and issues via REST API.
//!
//! Endpoints:
//! - Repos: GET /search/repositories?q=<q>&sort=stars&per_page=<N>
//! - Issues: GET /search/issues?q=<q>+repo:<owner>/<repo>&sort=updated&per_page=<N>
//!
//! Authentication:
//! - Optional token increases rate limit from 60/hr to 5000/hr.
//! - User-Agent header is required by GitHub API.

use super::engine::{GitHubExtra, SearchEngine, SearchError, SearchResult, url_encode};

/// GitHub search mode: repositories or issues.
enum GitHubMode {
    Repos,
    Issues { repo: String },
}

pub struct GitHubEngine {
    client: reqwest::Client,
    mode: GitHubMode,
    token: Option<String>,
}

impl GitHubEngine {
    /// Create a GitHub engine for repository search.
    pub fn repos(client: reqwest::Client, token: Option<String>) -> Self {
        Self {
            client,
            mode: GitHubMode::Repos,
            token,
        }
    }

    /// Create a GitHub engine for issue search within a repo.
    pub fn issues(client: reqwest::Client, token: Option<String>, repo: String) -> Self {
        Self {
            client,
            mode: GitHubMode::Issues { repo },
            token,
        }
    }

    /// Build headers for GitHub API requests.
    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            format!("oxibrowser/{}", env!("CARGO_PKG_VERSION"))
                .parse()
                .unwrap(),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            "application/vnd.github.v3+json".parse().unwrap(),
        );
        if let Some(token) = &self.token {
            let auth = format!("Bearer {token}");
            headers.insert(reqwest::header::AUTHORIZATION, auth.parse().unwrap());
        }
        headers
    }
}

#[async_trait::async_trait]
impl SearchEngine for GitHubEngine {
    fn name(&self) -> &'static str {
        match self.mode {
            GitHubMode::Repos => "GitHub",
            GitHubMode::Issues { .. } => "GitHub Issues",
        }
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        match &self.mode {
            GitHubMode::Repos => self.search_repos(query, max_results).await,
            GitHubMode::Issues { repo } => self.search_issues(query, repo, max_results).await,
        }
    }
}

impl GitHubEngine {
    /// Search GitHub repositories.
    async fn search_repos(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let encoded = url_encode(query);
        let per_page = max_results.min(30);
        let url = format!(
            "https://api.github.com/search/repositories?q={encoded}&sort=stars&per_page={per_page}"
        );

        let response = self
            .client
            .get(&url)
            .headers(self.headers())
            .send()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;

        check_github_rate_limit(&response)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SearchError::Network(format!(
                "GitHub API returned HTTP {status}: {body}"
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;

        parse_github_repos(&bytes)
    }

    /// Search GitHub issues within a repository.
    async fn search_issues(
        &self,
        query: &str,
        repo: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let encoded = url_encode(query);
        let per_page = max_results.min(30);
        let url = format!(
            "https://api.github.com/search/issues?q={encoded}+repo:{repo}&sort=updated&per_page={per_page}"
        );

        let response = self
            .client
            .get(&url)
            .headers(self.headers())
            .send()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;

        check_github_rate_limit(&response)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SearchError::Network(format!(
                "GitHub API returned HTTP {status}: {body}"
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;

        parse_github_issues(&bytes)
    }
}

/// Check for GitHub rate limit headers and return RateLimited error if triggered.
fn check_github_rate_limit(response: &reqwest::Response) -> Result<(), SearchError> {
    if response.status() == reqwest::StatusCode::FORBIDDEN
        || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        let remaining = response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("0");
        let reset = response
            .headers()
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown");
        return Err(SearchError::RateLimited(format!(
            "GitHub API rate limit exceeded ({remaining} remaining, resets at {reset})"
        )));
    }
    Ok(())
}

/// Parse GitHub repository search response.
///
/// Response structure:
/// ```json
/// {
///   "total_count": 42,
///   "items": [
///     {
///       "full_name": "owner/repo",
///       "html_url": "https://github.com/...",
///       "description": "...",
///       "stargazers_count": 128,
///       "forks_count": 12,
///       "language": "Rust",
///       "topics": ["browser", "cdp"],
///       "updated_at": "2026-06-05T12:12:41Z"
///     }
///   ]
/// }
/// ```
fn parse_github_repos(data: &[u8]) -> Result<Vec<SearchResult>, SearchError> {
    let parsed: serde_json::Value = serde_json::from_slice(data)
        .map_err(|e| SearchError::Parse(format!("GitHub repos: {e}")))?;

    let items = parsed
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or_else(|| SearchError::Parse("GitHub: missing items array".into()))?;

    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let obj = match item.as_object() {
            Some(o) => o,
            None => continue,
        };

        let full_name = obj
            .get("full_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let html_url = obj
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = obj
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let stars = obj
            .get("stargazers_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let forks = obj.get("forks_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let language = obj
            .get("language")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let topics = obj
            .get("topics")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let updated_at = obj
            .get("updated_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if full_name.is_empty() {
            continue;
        }

        results.push(SearchResult {
            title: full_name,
            url: html_url,
            snippet: description,
            source: "GitHub".into(),
            extra: Some(GitHubExtra {
                stars,
                forks,
                language,
                topics,
                updated_at,
            }),
        });
    }

    Ok(results)
}

/// Parse GitHub issue search response.
///
/// Response structure:
/// ```json
/// {
///   "total_count": 42,
///   "items": [
///     {
///       "title": "Memory leak in fetch",
///       "html_url": "https://github.com/...",
///       "state": "open",
///       "comments": 3,
///       "updated_at": "2026-06-05T12:12:41Z",
///       "user": {"login": "author"},
///       "labels": [{"name": "bug"}]
///     }
///   ]
/// }
/// ```
fn parse_github_issues(data: &[u8]) -> Result<Vec<SearchResult>, SearchError> {
    let parsed: serde_json::Value = serde_json::from_slice(data)
        .map_err(|e| SearchError::Parse(format!("GitHub issues: {e}")))?;

    let items = parsed
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or_else(|| SearchError::Parse("GitHub: missing items array".into()))?;

    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let obj = match item.as_object() {
            Some(o) => o,
            None => continue,
        };

        let title = obj
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let url = obj
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let state = obj.get("state").and_then(|v| v.as_str()).unwrap_or("");

        // Build snippet: "[#123] state: title"
        let number = obj.get("number").and_then(|v| v.as_u64()).unwrap_or(0);
        let user = obj
            .get("user")
            .and_then(|v| v.as_object())
            .and_then(|u| u.get("login"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let labels: Vec<&str> = obj
            .get("labels")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.get("name").and_then(|v| v.as_str()))
                    .collect()
            })
            .unwrap_or_default();

        let label_str = if labels.is_empty() {
            String::new()
        } else {
            format!(" [{}]", labels.join(", "))
        };

        let comments = obj.get("comments").and_then(|v| v.as_u64()).unwrap_or(0);
        let snippet = format!("#{number} by {user} · {state}{label_str} · {comments} comments");

        if title.is_empty() {
            continue;
        }

        results.push(SearchResult {
            title,
            url,
            snippet,
            source: "GitHub Issues".into(),
            extra: None,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_github_repos_basic() {
        let data = json!({
            "total_count": 1,
            "items": [{
                "full_name": "a7garden/oxibrowser",
                "html_url": "https://github.com/a7garden/oxibrowser",
                "description": "Headless browser in pure Rust",
                "stargazers_count": 128,
                "forks_count": 12,
                "language": "Rust",
                "topics": ["browser", "cdp"],
                "updated_at": "2026-06-05T12:12:41Z"
            }]
        });
        let bytes = serde_json::to_vec(&data).unwrap();
        let results = parse_github_repos(&bytes).unwrap();
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.title, "a7garden/oxibrowser");
        assert_eq!(r.url, "https://github.com/a7garden/oxibrowser");
        assert_eq!(r.snippet, "Headless browser in pure Rust");
        assert_eq!(r.source, "GitHub");
        let extra = r.extra.as_ref().unwrap();
        assert_eq!(extra.stars, 128);
        assert_eq!(extra.forks, 12);
        assert_eq!(extra.language.as_deref(), Some("Rust"));
        assert_eq!(extra.topics, vec!["browser", "cdp"]);
    }

    #[test]
    fn test_parse_github_repos_empty() {
        let data = json!({"total_count": 0, "items": []});
        let bytes = serde_json::to_vec(&data).unwrap();
        let results = parse_github_repos(&bytes).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_github_issues_basic() {
        let data = json!({
            "total_count": 1,
            "items": [{
                "title": "Memory leak in fetch",
                "html_url": "https://github.com/a7garden/oxibrowser/issues/42",
                "number": 42,
                "state": "open",
                "comments": 3,
                "updated_at": "2026-06-05T12:12:41Z",
                "user": {"login": "developer"},
                "labels": [{"name": "bug"}, {"name": "performance"}]
            }]
        });
        let bytes = serde_json::to_vec(&data).unwrap();
        let results = parse_github_issues(&bytes).unwrap();
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.title, "Memory leak in fetch");
        assert_eq!(r.url, "https://github.com/a7garden/oxibrowser/issues/42");
        assert!(r.snippet.contains("bug"));
        assert!(r.snippet.contains("performance"));
        assert!(r.snippet.contains("open"));
        assert_eq!(r.source, "GitHub Issues");
    }

    #[test]
    fn test_parse_github_issues_no_labels() {
        let data = json!({
            "total_count": 1,
            "items": [{
                "title": "Simple issue",
                "html_url": "https://github.com/a7garden/oxibrowser/issues/1",
                "number": 1,
                "state": "closed",
                "comments": 0,
                "updated_at": "2026-01-01T00:00:00Z",
                "user": {"login": "dev"},
                "labels": []
            }]
        });
        let bytes = serde_json::to_vec(&data).unwrap();
        let results = parse_github_issues(&bytes).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].snippet.contains("closed"));
        assert!(!results[0].snippet.contains('[')); // no labels
    }
}
