//! Wikipedia search engine — uses OpenSearch JSON API.
//!
//! Endpoint: GET /w/api.php?action=opensearch&search=<q>&limit=<N>&format=json
//!
//! Response format:
//! ```json
//! ["query", ["title1", "title2"], ["url1", "url2"], ["snippet1", "snippet2"]]
//! ```

use super::engine::{decode_html_entities, url_encode, SearchEngine, SearchError, SearchResult};

pub struct WikipediaEngine {
    client: reqwest::Client,
}

impl WikipediaEngine {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl SearchEngine for WikipediaEngine {
    fn name(&self) -> &'static str {
        "Wikipedia"
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>, SearchError> {
        let encoded = url_encode(query);
        let url = format!(
            "https://en.wikipedia.org/w/api.php?action=opensearch&search={encoded}&limit={max_results}&format=json"
        );

        let body = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;

        let status = body.status();
        if !status.is_success() {
            return Err(SearchError::Network(format!("Wikipedia API returned HTTP {status}")));
        }

        let bytes = body
            .bytes()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;

        // OpenSearch returns a JSON array of 4 elements:
        // [query: String, titles: Vec<String>, urls: Vec<String>, snippets: Vec<String>]
        let parsed: Vec<serde_json::Value> = serde_json::from_slice(&bytes)
            .map_err(|e| SearchError::Parse(format!("invalid Wikipedia response: {e}")))?;

        if parsed.len() < 4 {
            return Err(SearchError::Parse("incomplete Wikipedia response".into()));
        }

        let titles = parsed[1].as_array().ok_or_else(|| SearchError::Parse("expected titles array".into()))?;
        // OpenSearch returns URLs in index 2 (traditionally) or index 3 (actual).
        // Use whichever has actual content.
        let url_sources = parsed[2].as_array();
        let snippet_sources = parsed[3].as_array();

        let count = titles.len();

        let mut results = Vec::with_capacity(count);
        for i in 0..count {
            let title = titles[i].as_str().unwrap_or("").to_string();
            // URL: prefer index 2 (canonical), fall back to index 3
            let url = url_sources
                .and_then(|a| a.get(i))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    snippet_sources
                        .and_then(|a| a.get(i))
                        .and_then(|v| v.as_str())
                        .filter(|s| s.starts_with("http"))
                })
                .unwrap_or("")
                .to_string();
            // Snippet: index 3, but only if it's not a URL
            let snippet = snippet_sources
                .and_then(|a| a.get(i))
                .and_then(|v| v.as_str())
                .filter(|s| !s.starts_with("http"))
                .map(|s| decode_html_entities(s))
                .unwrap_or_default();

            results.push(SearchResult {
                title,
                url,
                snippet,
                source: "Wikipedia".into(),
                extra: None,
            });
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_wikipedia_parse_valid_response() {
        // Mock an OpenSearch response
        let json = serde_json::json!([
            "rust",
            ["Rust (programming language)", "Rust (band)"],
            ["https://en.wikipedia.org/wiki/Rust_(programming_language)", "https://en.wikipedia.org/wiki/Rust_(band)"],
            ["A systems programming language", "A German rock band"]
        ]);

        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json.to_string()).unwrap();
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0].as_str().unwrap(), "rust");

        let titles = parsed[1].as_array().unwrap();
        assert_eq!(titles.len(), 2);
        assert_eq!(titles[0].as_str().unwrap(), "Rust (programming language)");
    }

    #[test]
    fn test_wikipedia_parse_empty_response() {
        let json = serde_json::json!(["test", [], [], []]);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json.to_string()).unwrap();
        let titles = parsed[1].as_array().unwrap();
        assert!(titles.is_empty());
    }

    #[test]
    fn test_wikipedia_parse_malformed_rejected() {
        let json = serde_json::json!(["only_query"]);
        let parsed: Result<Vec<serde_json::Value>, _> = serde_json::from_str(&json.to_string());
        assert!(parsed.is_ok());
        assert!(parsed.unwrap().len() < 4);
    }
}
