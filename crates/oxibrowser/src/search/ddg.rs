//! DuckDuckGo search engine.
//!
//! Two-tier strategy:
//! 1. Primary: `https://lite.duckduckgo.com/lite/?q=<query>&kl=us-en` — lightweight HTML
//! 2. Fallback: `https://api.duckduckgo.com/?q=<query>&format=json&no_html=1` — Instant Answer API
//!
//! The lite HTML response is a simple table with `<tr class="result">` rows.
//! If the lite page returns zero results, we fall through to the JSON API.

use super::engine::{decode_html_entities, url_encode, SearchEngine, SearchError, SearchResult};

pub struct DuckDuckGoEngine {
    client: reqwest::Client,
}

impl DuckDuckGoEngine {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// Search via the lite HTML endpoint.
    async fn search_lite(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>, SearchError> {
        let encoded = url_encode(query);
        let url = format!("https://lite.duckduckgo.com/lite/?q={encoded}&kl=us-en");

        let body = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;

        let status = body.status();
        if !status.is_success() {
            return Err(SearchError::Network(format!("DDG lite returned HTTP {status}")));
        }

        let html = body
            .text()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;

        // Detect CAPTCHA challenge
        if html.contains("Unfortunately, bots use DuckDuckGo")
            || html.contains("Please complete the following challenge")
            || html.contains("CaptchaChallenge")
        {
            tracing::warn!("DuckDuckGo lite CAPTCHA challenge detected");
            return Ok(Vec::new());
        }

        Ok(parse_ddg_lite_html(&html, max_results))
    }

    /// Fallback: Instant Answer JSON API.
    async fn search_instant_answer(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>, SearchError> {
        let encoded = url_encode(query);
        let url = format!("https://api.duckduckgo.com/?q={encoded}&format=json&no_html=1");

        let body = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;

        let status = body.status();
        if !status.is_success() {
            return Err(SearchError::Network(format!("DDG API returned HTTP {status}")));
        }

        let bytes = body
            .bytes()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;

        parse_ddg_instant_answer(&bytes, max_results)
    }
}

#[async_trait::async_trait]
impl SearchEngine for DuckDuckGoEngine {
    fn name(&self) -> &'static str {
        "DuckDuckGo"
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>, SearchError> {
        // Try lite HTML first
        let results = self.search_lite(query, max_results).await?;
        if !results.is_empty() {
            return Ok(results);
        }
        // Empty → try Instant Answer API
        self.search_instant_answer(query, max_results).await
    }
}

// ---------------------------------------------------------------------------
// Lite HTML parser
// ---------------------------------------------------------------------------

/// Parse DuckDuckGo Lite HTML and extract search results.
///
/// The lite page structure is a simple table:
/// ```html
/// <tr class="result">
///   <td class="result-snippet">Snippet text</td>
///   <td class="result-metadata">
///     <a rel="nofollow" href="URL">Title</a>
///   </td>
/// </tr>
/// ```
fn parse_ddg_lite_html(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // Split by result rows
    for block in html.split(r#"<tr class="result">"#).skip(1) {
        if results.len() >= max_results {
            break;
        }

        // Extract title: find <a ...>TITLE</a>
        let (title, url) = extract_link_info(block);

        // Extract snippet: find <td class="result-snippet"> ... </td>
        let snippet = extract_between(block, r#"<td class="result-snippet">"#, "</td>")
            .map(|s| strip_html_tags(&s))
            .map(|s| decode_html_entities(&s))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        if url.is_empty() || title.is_empty() {
            continue;
        }

        results.push(SearchResult {
            title: decode_html_entities(&title),
            url,
            snippet,
            source: "DuckDuckGo".into(),
            extra: None,
        });
    }

    results
}

/// Extract href and text from an <a> tag in HTML.
/// Looks for `<a ... href="URL" ...>TITLE</a>`.
fn extract_link_info(block: &str) -> (String, String) {
    // Find <a with href
    let mut url = String::new();
    let mut title = String::new();

    // Find the first <a> tag
    if let Some(a_start) = block.find("<a ") {
        let after_a = &block[a_start + 3..];
        // Find href="..." or href='...'
        if let Some(href_start) = find_attr_start(after_a, "href") {
            let quote = after_a[href_start..].chars().next();
            if let Some(q) = quote {
                let q_str = q.to_string();
                let after_quote = &after_a[href_start + 1..];
                if let Some(quote_end) = after_quote.find(&q_str) {
                    url = after_quote[..quote_end].to_string();
                }
            }
        }

        // Find '>' after the <a tag
        if let Some(gt) = after_a.find('>') {
            let after_gt = &after_a[gt + 1..];
            if let Some(end_tag) = after_gt.find("</a>") {
                title = after_gt[..end_tag].to_string();
            }
        }
    }

    (title, url)
}

/// Find the position right after `attr=` in a substring.
/// Returns index of the start of the value (after the opening quote) if found.
fn find_attr_start(s: &str, attr: &str) -> Option<usize> {
    let pattern = format!(r#"{attr}="#);
    if let Some(pos) = s.find(&pattern) {
        Some(pos + pattern.len())
    } else {
        None
    }
}

/// Extract text between two markers in a string.
fn extract_between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start_pos = s.find(start)?;
    let after = &s[start_pos + start.len()..];
    let end_pos = after.find(end)?;
    Some(&after[..end_pos])
}

/// Strip HTML tags from text.
fn strip_html_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result.trim().to_string()
}

// ---------------------------------------------------------------------------
// Instant Answer JSON API parser
// ---------------------------------------------------------------------------

/// Parse DuckDuckGo Instant Answer JSON response.
///
/// Response structure (simplified):
/// ```json
/// {
///   "AbstractText": "summary text",
///   "AbstractURL": "https://...",
///   "AbstractSource": "Wikipedia",
///   "RelatedTopics": [
///     {"Text": "Title - snippet", "FirstURL": "https://..."},
///     {"Topics": [...]} // nested categories
///   ]
/// }
/// ```
fn parse_ddg_instant_answer(data: &[u8], max_results: usize) -> Result<Vec<SearchResult>, SearchError> {
    let parsed: serde_json::Value = serde_json::from_slice(data)
        .map_err(|e| SearchError::Parse(format!("DDG instant answer: {e}")))?;

    let obj = parsed.as_object().ok_or_else(|| SearchError::Parse("expected object".into()))?;
    let mut results = Vec::new();

    // Abstract result (if present)
    if let Some(abstract_text) = obj.get("AbstractText").and_then(|v| v.as_str()) {
        if !abstract_text.is_empty() {
            if let Some(abstract_url) = obj.get("AbstractURL").and_then(|v| v.as_str()) {
                if !abstract_url.is_empty() {
                    results.push(SearchResult {
                        title: obj.get("Heading")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        url: abstract_url.to_string(),
                        snippet: abstract_text.to_string(),
                        source: "DuckDuckGo".into(),
                        extra: None,
                    });
                }
            }
        }
    }

    if results.len() >= max_results {
        return Ok(results);
    }

    // Related topics
    if let Some(topics) = obj.get("RelatedTopics").and_then(|v| v.as_array()) {
        for topic in topics {
            if results.len() >= max_results {
                break;
            }
            if let Some(text) = topic.get("Text").and_then(|v| v.as_str()) {
                // Text format: "Title - snippet" or just "Title"
                let url = topic.get("FirstURL")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let title;
                let snippet;
                if let Some(dash) = text.find(" - ") {
                    title = text[..dash].to_string();
                    snippet = text[dash + 3..].to_string();
                } else {
                    title = text.to_string();
                    snippet = String::new();
                }
                results.push(SearchResult {
                    title: decode_html_entities(&title),
                    url,
                    snippet: decode_html_entities(&snippet),
                    source: "DuckDuckGo".into(),
                    extra: None,
                });
            }
            // Handle nested Topics (categories with subtopics)
            if let Some(subtopics) = topic.get("Topics").and_then(|v| v.as_array()) {
                for sub in subtopics {
                    if results.len() >= max_results {
                        break;
                    }
                    if let Some(text) = sub.get("Text").and_then(|v| v.as_str()) {
                        let url = sub.get("FirstURL")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let title;
                        let snippet;
                        if let Some(dash) = text.find(" - ") {
                            title = text[..dash].to_string();
                            snippet = text[dash + 3..].to_string();
                        } else {
                            title = text.to_string();
                            snippet = String::new();
                        }
                        results.push(SearchResult {
                            title: decode_html_entities(&title),
                            url,
                            snippet: decode_html_entities(&snippet),
                            source: "DuckDuckGo".into(),
                            extra: None,
                        });
                    }
                }
            }
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_strip_html_tags_simple() {
        assert_eq!(strip_html_tags("<b>bold</b>"), "bold");
    }

    #[test]
    fn test_strip_html_tags_mixed() {
        assert_eq!(strip_html_tags("hello <a href='x'>world</a>!"), "hello world!");
    }

    #[test]
    fn test_strip_html_tags_no_tags() {
        assert_eq!(strip_html_tags("plain text"), "plain text");
    }

    #[test]
    fn test_extract_between_found() {
        assert_eq!(
            extract_between("x <tag> y z</tag> end", "<tag>", "</tag>"),
            Some(" y z")
        );
    }

    #[test]
    fn test_extract_between_not_found() {
        assert_eq!(extract_between("hello", "x", "y"), None);
    }

    #[test]
    fn test_parse_ddg_instant_answer_abstract_only() {
        let data = json!({
            "AbstractText": "Rust is a systems programming language.",
            "AbstractURL": "https://www.rust-lang.org/",
            "Heading": "Rust (programming language)",
            "AbstractSource": "Wikipedia",
            "RelatedTopics": []
        });
        let bytes = serde_json::to_vec(&data).unwrap();
        let results = parse_ddg_instant_answer(&bytes, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust (programming language)");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert!(results[0].snippet.contains("systems programming"));
    }

    #[test]
    fn test_parse_ddg_instant_answer_empty() {
        let data = json!({
            "AbstractText": "",
            "AbstractURL": "",
            "RelatedTopics": []
        });
        let bytes = serde_json::to_vec(&data).unwrap();
        let results = parse_ddg_instant_answer(&bytes, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_link_extraction_simple() {
        let block = r#"<a rel="nofollow" href="https://example.com">Example Site</a>"#;
        let (title, url) = extract_link_info(block);
        assert_eq!(title, "Example Site");
        assert_eq!(url, "https://example.com");
    }

    #[test]
    fn test_link_extraction_extra_attrs() {
        let block = r#"<a class="result-link" href="https://test.org" rel="nofollow">Test</a>"#;
        let (title, url) = extract_link_info(block);
        assert_eq!(title, "Test");
        assert_eq!(url, "https://test.org");
    }
}
