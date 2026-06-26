//! High-fidelity content extraction: main-text (Readability-lite), structured
//! data (JSON-LD), Open Graph, tables, and improved Markdown — all derived from
//! a [`DomSnapshot`] with no HTTP dependency.
//!
//! This is the agent's #1 read path: turn an arbitrary page into clean text and
//! structured metadata. `frame.to_markdown()` stays a flat DOM dump; this module
//! scores the DOM to isolate the main content and renders it more usefully.

use crate::js::dom_snapshot::{DomNode, DomSnapshot};
use serde_json::Value;

/// Class/id substrings that signal boilerplate (nav, ads, page chrome).
const NEGATIVE_HINTS: &[&str] = &[
    "nav",
    "navbar",
    "menu",
    "footer",
    "header",
    "sidebar",
    "breadcrumb",
    "comment",
    "advert",
    "promo",
    "cookie",
    "popup",
    "modal",
    "banner",
    "share",
    "social",
    "subscribe",
    "related",
    "widget",
    "pagination",
];
/// Class/id substrings that signal main content.
const POSITIVE_HINTS: &[&str] = &[
    "article",
    "content",
    "main",
    "post",
    "story",
    "entry",
    "page-content",
    "body-content",
    "blog",
    "prose",
];
/// Tags whose subtrees carry no readable content.
const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "head", "iframe", "form", "button", "select",
    "canvas", "map", "area",
];
/// Block-level tags that start a new line in plain-text rendering.
const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "main",
    "header",
    "footer",
    "aside",
    "nav",
    "ul",
    "ol",
    "li",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "pre",
    "table",
    "tr",
    "br",
    "hr",
];

/// Minimum visible-text length (chars) for a node to be considered a content
/// container. Keeps tiny chrome nodes out of the scoring race.
const MIN_CONTENT_CHARS: usize = 25;

/// Readability-style content extractor operating on a borrowed DOM snapshot.
pub struct ContentExtractor<'a> {
    snap: &'a DomSnapshot,
}

/// Everything [`ContentExtractor`] can pull out of a page, in one struct.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtractedContent {
    pub title: String,
    pub url: String,
    pub main_text: String,
    pub markdown: String,
    pub opengraph: Value,
    pub structured: Vec<Value>,
    pub tables: Vec<String>,
}

impl<'a> ContentExtractor<'a> {
    /// Wrap a DOM snapshot for extraction.
    pub fn from_snapshot(snap: &'a DomSnapshot) -> Self {
        Self { snap }
    }

    /// Run every extractor and return the combined result.
    pub fn extract(&self) -> ExtractedContent {
        ExtractedContent {
            title: self.snap.title.clone(),
            url: self.snap.url.clone(),
            main_text: self.main_text(),
            markdown: self.markdown(),
            opengraph: self.opengraph(),
            structured: self.structured(),
            tables: self.tables(),
        }
    }

    fn node(&self, id: u32) -> Option<&'a DomNode> {
        self.snap.nodes.get(&id)
    }

    fn tag(&self, node: &DomNode) -> String {
        node.tag.to_lowercase()
    }

    fn is_skipped(&self, node: &DomNode) -> bool {
        if node.node_type == 1 && SKIP_TAGS.contains(&self.tag(node).as_str()) {
            return true;
        }
        self.is_hidden(node)
    }

    fn is_hidden(&self, node: &DomNode) -> bool {
        if node.attributes.contains_key("hidden") {
            return true;
        }
        if node.attributes.get("aria-hidden").map(|s| s.as_str()) == Some("true") {
            return true;
        }
        if let Some(style) = node.attributes.get("style") {
            let compact = style.to_lowercase().replace(' ', "");
            if compact.contains("display:none") || compact.contains("visibility:hidden") {
                return true;
            }
        }
        false
    }

    /// Plain text of a subtree, skipping non-content tags and hidden nodes.
    fn visible_text(&self, node_id: u32) -> String {
        let mut s = String::new();
        self.visible_text_into(node_id, &mut s);
        normalize_ws(&s)
    }

    fn visible_text_into(&self, node_id: u32, s: &mut String) {
        if let Some(node) = self.node(node_id) {
            match node.node_type {
                3 => {
                    s.push_str(&node.text_content);
                    s.push(' ');
                }
                1 if !self.is_skipped(node) => {
                    for &c in &node.children {
                        self.visible_text_into(c, s);
                    }
                }
                _ => {}
            }
        }
    }

    /// Plain text with line breaks at block boundaries (for `main_text`).
    fn block_text(&self, node_id: u32) -> String {
        let mut s = String::new();
        self.block_text_into(node_id, &mut s);
        normalize_lines(&s)
    }

    fn block_text_into(&self, node_id: u32, s: &mut String) {
        if let Some(node) = self.node(node_id) {
            match node.node_type {
                3 => {
                    s.push_str(&node.text_content);
                    s.push(' ');
                }
                1 if !self.is_skipped(node) => {
                    let t = self.tag(node);
                    if !s.is_empty() && BLOCK_TAGS.contains(&t.as_str()) {
                        s.push('\n');
                    }
                    for &c in &node.children {
                        self.block_text_into(c, s);
                    }
                    if t == "br" {
                        s.push('\n');
                    }
                }
                _ => {}
            }
        }
    }

    fn class_or_id(&self, node: &DomNode) -> String {
        let mut buf = String::new();
        if let Some(c) = node.attributes.get("class") {
            buf.push_str(c);
            buf.push(' ');
        }
        if let Some(i) = node.attributes.get("id") {
            buf.push_str(i);
        }
        buf.to_lowercase()
    }

    /// Multiplier from class/id hints (positive boosts, negative suppresses).
    fn hint_score(&self, node: &DomNode) -> f64 {
        let h = self.class_or_id(node);
        let mut score = 1.0_f64;
        for neg in NEGATIVE_HINTS {
            if h.contains(neg) {
                score *= 0.2;
            }
        }
        for pos in POSITIVE_HINTS {
            if h.contains(pos) {
                score *= 1.5;
            }
        }
        score
    }

    /// Total visible text length of a node's anchor (`<a>`) descendants.
    fn link_text_len(&self, node_id: u32) -> usize {
        let mut total = 0usize;
        let mut stack = vec![node_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.node(id)
                && node.node_type == 1
            {
                if self.is_skipped(node) {
                    continue;
                }
                if self.tag(node) == "a" {
                    total += self.visible_text(node.id).chars().count();
                    continue; // don't descend into nested anchors
                }
                for &c in &node.children {
                    stack.push(c);
                }
            }
        }
        total
    }

    /// Pick the highest-scoring content container (Readability-lite).
    fn main_container(&self) -> Option<u32> {
        let start = self.snap.body_id.or(Some(self.snap.root_id))?;
        let mut best: Option<(u32, f64)> = None;
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.node(id)
                && node.node_type == 1
                && !self.is_skipped(node)
            {
                let text_len = self.visible_text(node.id).chars().count();
                if text_len >= MIN_CONTENT_CHARS {
                    let link_len = self.link_text_len(node.id).min(text_len);
                    let link_density = link_len as f64 / text_len as f64;
                    let tag_bonus = match self.tag(node).as_str() {
                        "article" | "main" => 1.3,
                        "aside" | "nav" => 0.3,
                        _ => 1.0,
                    };
                    let score =
                        text_len as f64 * self.hint_score(node) * tag_bonus * (1.0 - link_density);
                    if score > best.map(|(_, s)| s).unwrap_or(0.0) {
                        best = Some((node.id, score));
                    }
                }
                for &c in &node.children {
                    stack.push(c);
                }
            }
        }
        best.map(|(id, _)| id)
    }

    /// Readability-style main text: text of the best container, paragraph-broken.
    pub fn main_text(&self) -> String {
        self.main_container()
            .or(self.snap.body_id)
            .map(|id| self.block_text(id))
            .unwrap_or_default()
    }

    /// Open Graph + Twitter card metadata as a JSON object.
    pub fn opengraph(&self) -> Value {
        let mut obj = serde_json::Map::new();
        for (k, v) in self.snap.meta_tags() {
            let kl = k.to_lowercase();
            if kl.starts_with("og:") || kl.starts_with("twitter:") {
                obj.insert(k, Value::String(v));
            }
        }
        Value::Object(obj)
    }

    /// All JSON-LD blocks (`<script type="application/ld+json">`) parsed.
    pub fn structured(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for node in self.snap.nodes.values() {
            if node.node_type == 1
                && self.tag(node) == "script"
                && node.attributes.get("type").map(|s| s.as_str()) == Some("application/ld+json")
            {
                // Scripts are skipped by `visible_text`; read their pre-computed
                // deep `text_content` directly to recover the JSON-LD payload.
                let raw = node.text_content.trim();
                if let Ok(arr) = serde_json::from_str::<Vec<Value>>(raw) {
                    out.extend(arr);
                } else if let Ok(val) = serde_json::from_str::<Value>(raw) {
                    out.push(val);
                }
            }
        }
        out
    }

    /// Every `<table>` rendered as a GitHub-flavored Markdown table.
    pub fn tables(&self) -> Vec<String> {
        self.snap
            .nodes
            .values()
            .filter(|n| n.node_type == 1 && n.tag.eq_ignore_ascii_case("table"))
            .filter_map(|n| self.render_table(n.id))
            .collect()
    }

    fn render_table(&self, table_id: u32) -> Option<String> {
        let mut rows: Vec<Vec<String>> = Vec::new();
        self.collect_rows(table_id, &mut rows);
        if rows.is_empty() {
            return None;
        }
        let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut md = String::new();
        let header = &rows[0];
        md.push('|');
        for i in 0..width {
            md.push_str(&format!(
                " {} |",
                header.get(i).map(|s| s.as_str()).unwrap_or("")
            ));
        }
        md.push_str("\n|");
        for _ in 0..width {
            md.push_str(" --- |");
        }
        md.push('\n');
        for row in rows.iter().skip(1) {
            md.push('|');
            for i in 0..width {
                md.push_str(&format!(
                    " {} |",
                    row.get(i).map(|s| s.as_str()).unwrap_or("")
                ));
            }
            md.push('\n');
        }
        Some(md.trim_end().to_string())
    }

    fn collect_rows(&self, node_id: u32, rows: &mut Vec<Vec<String>>) {
        let Some(node) = self.node(node_id) else {
            return;
        };
        if node.node_type != 1 || self.is_skipped(node) {
            return;
        }
        if self.tag(node) == "tr" {
            let mut cells = Vec::new();
            for &c in &node.children {
                if let Some(cn) = self.node(c) {
                    let ct = self.tag(cn);
                    if ct == "td" || ct == "th" {
                        cells.push(self.visible_text(cn.id));
                    }
                }
            }
            if !cells.is_empty() {
                rows.push(cells);
            }
            return; // a row's children are cells, not nested rows
        }
        for &c in &node.children {
            self.collect_rows(c, rows);
        }
    }

    /// Improved Markdown rendering of the main content only (no nav/footer/ads).
    pub fn markdown(&self) -> String {
        let root = self
            .main_container()
            .or(self.snap.body_id)
            .unwrap_or(self.snap.root_id);
        let mut s = String::new();
        self.markdown_into(root, &mut s);
        // Light cleanup: drop trailing spaces per line without collapsing
        // whitespace inside lines (preserves code blocks).
        s.lines()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }

    fn markdown_into(&self, node_id: u32, s: &mut String) {
        let Some(node) = self.node(node_id) else {
            return;
        };
        match node.node_type {
            3 => {
                let t = node.text_content.trim();
                if !t.is_empty() {
                    s.push_str(t);
                    s.push(' ');
                }
            }
            1 => {
                if self.is_skipped(node) {
                    return;
                }
                let t = self.tag(node);
                match t.as_str() {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        let level = t[1..].parse::<usize>().unwrap_or(1);
                        if !s.is_empty() {
                            s.push_str("\n\n");
                        }
                        s.push_str(&"#".repeat(level));
                        s.push(' ');
                        s.push_str(&self.visible_text(node.id));
                        s.push_str("\n\n");
                    }
                    "p" => {
                        if !s.is_empty() {
                            s.push_str("\n\n");
                        }
                        for &c in &node.children {
                            self.markdown_into(c, s);
                        }
                        s.push('\n');
                    }
                    "li" => {
                        s.push_str("\n- ");
                        for &c in &node.children {
                            self.markdown_into(c, s);
                        }
                    }
                    "br" => s.push('\n'),
                    "hr" => s.push_str("\n\n---\n\n"),
                    "blockquote" => {
                        if !s.is_empty() {
                            s.push('\n');
                        }
                        s.push_str("> ");
                        for &c in &node.children {
                            self.markdown_into(c, s);
                        }
                        s.push('\n');
                    }
                    "pre" => {
                        s.push_str("\n```\n");
                        s.push_str(&self.visible_text(node.id));
                        s.push_str("\n```\n");
                    }
                    "img" => {
                        let alt = node.attributes.get("alt").cloned().unwrap_or_default();
                        let src = node.attributes.get("src").cloned().unwrap_or_default();
                        s.push_str(&format!("![{alt}]({src})"));
                    }
                    "a" => {
                        let href = node.attributes.get("href").cloned().unwrap_or_default();
                        let text = self.visible_text(node.id);
                        if !text.is_empty() {
                            s.push_str(&format!("[{text}]({href})"));
                        }
                    }
                    "table" => {
                        if let Some(md) = self.render_table(node.id) {
                            if !s.is_empty() {
                                s.push('\n');
                            }
                            s.push_str(&md);
                            s.push('\n');
                        }
                    }
                    _ => {
                        for &c in &node.children {
                            self.markdown_into(c, s);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Collapse all whitespace runs to a single space (for flat `visible_text`).
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Collapse blank lines (max one) and trailing spaces, keeping line structure.
fn normalize_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank <= 1 {
                out.push('\n');
            }
        } else {
            blank = 0;
            let collapsed: String = line
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&collapsed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;
    use url::Url;

    fn snapshot_from(html: &str) -> DomSnapshot {
        let url = Url::parse("https://example.com").unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let frame = rt.block_on(Frame::from_html(url, html)).unwrap();
        DomSnapshot::from_frame(&frame)
    }

    #[test]
    fn main_text_excludes_nav_and_ads() {
        let html = r#"<html><head><title>News</title></head><body>
          <nav class="navbar"><a href="/">Home</a> <a href="/about">About</a></nav>
          <div class="ad-banner">Buy now! Click here for deals!</div>
          <article class="article-content">
            <h1>Real Headline</h1>
            <p>This is the important article body content that should be extracted.</p>
            <p>It has multiple paragraphs of meaningful text for the reader to enjoy.</p>
          </article>
        </body></html>"#;
        let snap = snapshot_from(html);
        let ext = ContentExtractor::from_snapshot(&snap);
        let text = ext.main_text();
        assert!(
            text.contains("important article body content"),
            "missing body: {text}"
        );
        assert!(
            text.contains("multiple paragraphs"),
            "missing 2nd para: {text}"
        );
        assert!(!text.contains("Buy now"), "included ad: {text}");
        assert!(!text.contains("Home"), "included nav: {text}");
    }

    #[test]
    fn structured_parses_jsonld() {
        let html = r#"<html><body>
          <script type="application/ld+json">{"@type":"Product","name":"Widget","price":"9.99"}</script>
          <p>Some content here that is long enough to matter for the page.</p>
        </body></html>"#;
        let snap = snapshot_from(html);
        let ext = ContentExtractor::from_snapshot(&snap);
        let s = ext.structured();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0]["@type"].as_str(), Some("Product"));
        assert_eq!(s[0]["name"].as_str(), Some("Widget"));
    }

    #[test]
    fn opengraph_collects_og_meta() {
        let html = r#"<html><head>
          <meta property="og:title" content="My Page">
          <meta property="og:type" content="website">
          <meta name="description" content="should be ignored">
        </head><body><p>enough body text to pass thresholds comfortably here.</p></body></html>"#;
        let snap = snapshot_from(html);
        let ext = ContentExtractor::from_snapshot(&snap);
        let og = ext.opengraph();
        assert_eq!(og["og:title"].as_str(), Some("My Page"));
        assert_eq!(og["og:type"].as_str(), Some("website"));
        assert!(og.get("description").is_none(), "non-og meta leaked: {og}");
    }

    #[test]
    fn tables_render_as_markdown() {
        let html = r#"<html><body><table>
          <tr><th>Name</th><th>Age</th></tr>
          <tr><td>Ada</td><td>36</td></tr>
        </table></body></html>"#;
        let snap = snapshot_from(html);
        let ext = ContentExtractor::from_snapshot(&snap);
        let tables = ext.tables();
        assert_eq!(tables.len(), 1);
        assert!(
            tables[0].contains("| Name | Age |"),
            "header: {}",
            tables[0]
        );
        assert!(tables[0].contains("| --- |"), "separator: {}", tables[0]);
        assert!(
            tables[0].contains("| Ada | 36 |"),
            "data row: {}",
            tables[0]
        );
    }

    #[test]
    fn markdown_excludes_boilerplate() {
        let html = r#"<html><body>
          <nav class="nav"><a href="/">Home</a> <a href="/x">Other</a></nav>
          <article class="content">
            <h1>Title</h1>
            <p>Body text here that is long enough to be recognized as the main article content.</p>
          </article>
        </body></html>"#;
        let snap = snapshot_from(html);
        let ext = ContentExtractor::from_snapshot(&snap);
        let md = ext.markdown();
        assert!(md.contains("# Title"), "missing heading: {md}");
        assert!(md.contains("Body text here"), "missing body: {md}");
        assert!(!md.contains("Home"), "included nav: {md}");
    }

    #[test]
    fn extract_returns_all_fields() {
        let html = r#"<html><head>
          <title>T</title>
          <meta property="og:title" content="OG">
          <script type="application/ld+json">{"@type":"Article"}</script>
        </head><body>
          <article><p>Article body content long enough to clear the threshold easily.</p></article>
        </body></html>"#;
        let snap = snapshot_from(html);
        let c = ContentExtractor::from_snapshot(&snap).extract();
        assert_eq!(c.title, "T");
        assert!(!c.main_text.is_empty());
        assert!(!c.markdown.is_empty());
        assert_eq!(c.opengraph["og:title"].as_str(), Some("OG"));
        assert_eq!(c.structured.len(), 1);
        assert!(c.url.contains("example.com"));
    }
}
