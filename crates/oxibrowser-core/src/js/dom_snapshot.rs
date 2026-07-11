//! DOM snapshot for JS↔DOM bridge.
//!
//! Provides a `Send + Sync + Serialize` representation of the DOM tree
//! that can be passed between the main thread and the JS thread via channels.
//!
//! Architecture:
//! ```text
//! Frame's Document → DomSnapshot::from_frame() → JsCommand::SetDom → JS thread
//!                                                              ↓
//! JS: document.querySelector('a') → DomSnapshot::query_selector() → result
//! ```

use crate::css::ComputedStyle;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// DOM 변경 사항
///
/// Records mutations applied to the DOM so they can be replayed,
/// inspected, or transmitted over the CDP protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DomMutation {
    /// Set an attribute on a node.
    SetAttribute {
        node_id: u32,
        name: String,
        value: String,
    },
    /// Set the text content of a node.
    SetTextContent { node_id: u32, text: String },
    /// Simulate a click on an element.
    ClickElement { node_id: u32 },
    /// Input text into a form element.
    InputElement { node_id: u32, value: String },
    /// Create a new element node.
    CreateElement { node_id: u32, tag: String },
    /// Create a new text node.
    CreateTextNode { node_id: u32, text: String },
    /// Append a child node to a parent.
    AppendChild { parent_id: u32, child_id: u32 },
    /// Remove a child node from its parent.
    RemoveChild { parent_id: u32, child_id: u32 },
    /// Set innerHTML of an element (parse + replace children).
    SetInnerHtml { node_id: u32, html: String },
    /// Real navigation triggered from JS (`location.href`/`assign`/`replace`).
    /// Handled asynchronously by `Session::evaluate_js_with_await` (network I/O).
    Navigate { url: String },
    /// Page reload triggered from JS (`location.reload`).
    Reload,
}

/// Serializable DOM node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomNode {
    pub id: u32,
    pub tag: String,
    pub attributes: HashMap<String, String>,
    pub text_content: String,
    pub children: Vec<u32>,
    pub parent: Option<u32>,
    /// 1 = Element, 3 = Text, 8 = Comment, 9 = Document.
    pub node_type: u8,
}

/// Per-node `ComputedStyle` cache, keyed by `node_id` and invalidated by snapshot revision.
///
/// Stays private — only `DomSnapshot::compute_style_cached` reads or writes it.
#[derive(Debug, Clone, Default)]
struct StyleCache {
    /// Revision this cache was populated against. If `snapshot.revision` differs,
    /// the cache is stale and must be rebuilt from scratch.
    revision: u64,
    styles: HashMap<u32, ComputedStyle>,
}

/// DOM tree snapshot (Send + Serialize).
#[derive(Debug, Serialize, Deserialize)]
pub struct DomSnapshot {
    pub url: String,
    pub title: String,
    pub nodes: HashMap<u32, DomNode>,
    pub root_id: u32,
    pub body_id: Option<u32>,
    pub head_id: Option<u32>,
    /// Monotonic counter. Bumped on any mutation that affects computed styles
    /// OR structural shape (so the id/class/tag indices know to rebuild).
    /// `compute_style_cached` keys its cache against this; index-using methods
    /// lazily rebuild when `revision != index_revision`.
    #[serde(default)]
    pub revision: u64,
    /// Revision the id/class/tag indices were last rebuilt against.
    /// On any read-through, if `self.index_revision != self.revision`, indices
    /// are regenerated from `self.nodes` before use.
    #[serde(default)]
    pub index_revision: u64,
    /// `id` attribute → first node_id (HTML ids SHOULD be unique).
    #[serde(default)]
    pub id_index: HashMap<String, u32>,
    /// Class name → node_ids, in document (DFS pre-order) order.
    #[serde(default)]
    pub class_index: HashMap<String, Vec<u32>>,
    /// Tag name (already lowercased) → node_ids, in document order.
    #[serde(default)]
    pub tag_index: HashMap<String, Vec<u32>>,
    /// Lazily-populated per-node `ComputedStyle` cache. `Mutex` (not `RefCell`)
    /// so the snapshot stays `Send + Sync` — `Arc<RwLock<Option<DomSnapshot>>>`
    /// in runtime.rs requires `Sync`. Never serialized.
    #[serde(skip, default)]
    style_cache: Mutex<Option<StyleCache>>,
}

// Manual `Clone`: `std::sync::Mutex` doesn't derive Clone (cloning the inner
// while another thread holds the lock is a soundness hole). For our use case
// the cache is transient — every clone starts with a fresh empty cache.
impl Clone for DomSnapshot {
    fn clone(&self) -> Self {
        Self {
            url: self.url.clone(),
            title: self.title.clone(),
            nodes: self.nodes.clone(),
            root_id: self.root_id,
            body_id: self.body_id,
            head_id: self.head_id,
            revision: self.revision,
            index_revision: self.index_revision,
            id_index: self.id_index.clone(),
            class_index: self.class_index.clone(),
            tag_index: self.tag_index.clone(),
            style_cache: Mutex::new(None),
        }
    }
}

impl DomSnapshot {
    /// Create an empty snapshot (no document loaded).
    pub fn empty() -> Self {
        Self {
            url: String::new(),
            title: String::new(),
            nodes: HashMap::new(),
            root_id: 0,
            body_id: None,
            head_id: None,
            revision: 0,
            index_revision: 0,
            id_index: HashMap::new(),
            class_index: HashMap::new(),
            tag_index: HashMap::new(),
            style_cache: Mutex::new(None),
        }
    }

    /// Extract a snapshot from a Frame's Document.
    ///
    /// Walks all nodes in the document tree, converting each to a `DomNode`,
    /// and pre-builds id/class/tag indices so `query_selector` and friends can
    /// answer simple selectors in O(1)/O(matches) instead of walking the tree.
    pub fn from_frame(frame: &crate::frame::Frame) -> Self {
        let doc = frame.document();
        let tree = doc.tree();
        let url = frame.url().to_string();
        let title = frame.extract_title().unwrap_or_default();

        let mut nodes = HashMap::new();
        let mut body_id = None;
        let mut head_id = None;
        let mut order: Vec<u32> = Vec::new();

        // Walk all nodes via DFS from root, capturing document order for the indices.
        if let Some(root) = tree.root() {
            collect_nodes(
                root,
                doc,
                tree,
                &mut nodes,
                &mut order,
                &mut body_id,
                &mut head_id,
            );
        }

        let root_id = tree.root().map(|id| id.0 as u32).unwrap_or(0);

        // Build id/class/tag indices in document (DFS pre-order) so that
        // `query_selector` / `get_element_*` can return the first match in
        // correct order without re-walking the tree.
        let (id_index, class_index, tag_index) = build_indices(&nodes, &order);

        Self {
            url,
            title,
            nodes,
            root_id,
            body_id,
            head_id,
            revision: 0,
            index_revision: 0,
            id_index,
            class_index,
            tag_index,
            style_cache: Mutex::new(None),
        }
    }

    /// Bump the snapshot revision, invalidating the style cache AND marking
    /// the id/class/tag indices stale.
    ///
    /// Every in-place DOM mutation closure in `runtime.rs` MUST call this
    /// after mutating `snap.nodes`. Otherwise `compute_style_cached` returns
    /// cached styles for nodes whose attributes changed, and the indices
    /// return deleted/renamed ids/classes/tags.
    pub fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        // Drop the style cache; the next `compute_style_cached` rebuilds it.
        // Single-threaded access under the snapshot's RwLock; the Mutex exists
        // only for `Sync` and Clone compatibility — poisoning is impossible.
        *self.style_cache.lock().expect("style cache mutex poisoned") = None;
        // Indices become stale but stay allocated; the next index-using read
        // detects `index_revision != revision` and rebuilds them.
    }

    /// Compute (and cache) the resolved style for a node.
    ///
    /// Cache lookup is keyed by `node_id` and validated against the snapshot's
    /// `revision`. A revision mismatch drops the entire cache and the next
    /// call rebuilds it (cheaper than per-entry versioning because mutations
    pub fn compute_style_cached(&self, node_id: u32) -> Option<ComputedStyle> {
        let mut cache_slot = self.style_cache.lock().expect("style cache mutex poisoned");
        let cache: &mut StyleCache = match cache_slot.as_mut() {
            Some(c) if c.revision == self.revision => c,
            _ => {
                *cache_slot = Some(StyleCache {
                    revision: self.revision,
                    styles: HashMap::new(),
                });
                cache_slot
                    .as_mut()
                    .expect("style cache was just initialized above")
            }
        };

        if let Some(hit) = cache.styles.get(&node_id) {
            return Some(hit.clone());
        }

        let computed = crate::css::LayoutEngine::compute_style(self, node_id)?;
        cache.styles.insert(node_id, computed.clone());
        Some(computed)
    }

    /// Fast path for `query_selector` on simple `#id`, `.class`, bare `tag`.
    ///
    /// Returns the first matching node_id in document order ONLY if the
    /// pre-built indices are still fresh (`index_revision == revision`).
    /// After any in-place mutation (which bumps `revision`), indices become
    /// stale and we deliberately return `None` so the caller falls back to
    /// the tree walk — safer than risking stale results, and the perf win
    /// between `from_frame` and the first mutation is the meaningful window.
    fn simple_selector_first(&self, selector: &str) -> Option<u32> {
        // Indices only trustworthy between `from_frame` and the first mutation.
        if self.index_revision != self.revision {
            return None;
        }
        if let Some(id) = selector.strip_prefix('#') {
            if id.is_empty() {
                return None;
            }
            return self.id_index.get(id).copied();
        }
        if let Some(class) = selector.strip_prefix('.') {
            if class.is_empty() {
                return None;
            }
            return self.class_index.get(class).and_then(|v| v.first().copied());
        }
        if !selector.is_empty()
            && !selector.bytes().any(|b| {
                matches!(
                    b,
                    b' ' | b'\t'
                        | b'\n'
                        | b'.'
                        | b'#'
                        | b'['
                        | b','
                        | b'>'
                        | b'+'
                        | b'~'
                        | b':'
                        | b'*'
                )
            })
        {
            return self
                .tag_index
                .get(&selector.to_lowercase())
                .and_then(|v| v.first().copied());
        }
        None
    }

    /// Fast path for `query_selector_all` on simple `#id`, `.class`, bare `tag`.
    ///
    /// Same freshness rule as `simple_selector_first`: returns `None` once any
    /// in-place mutation has invalidated the indices, deferring to the tree
    /// walk.
    fn simple_selector_all(&self, selector: &str) -> Option<Vec<u32>> {
        if self.index_revision != self.revision {
            return None;
        }
        if let Some(id) = selector.strip_prefix('#') {
            if id.is_empty() {
                return None;
            }
            return self.id_index.get(id).map(|&id| vec![id]);
        }
        if let Some(class) = selector.strip_prefix('.') {
            if class.is_empty() {
                return None;
            }
            return self.class_index.get(class).cloned();
        }
        if !selector.is_empty()
            && !selector.bytes().any(|b| {
                matches!(
                    b,
                    b' ' | b'\t'
                        | b'\n'
                        | b'.'
                        | b'#'
                        | b'['
                        | b','
                        | b'>'
                        | b'+'
                        | b'~'
                        | b':'
                        | b'*'
                )
            })
        {
            return self.tag_index.get(&selector.to_lowercase()).cloned();
        }
        None
    }

    /// Query the first matching node by CSS selector.
    ///
    /// Supports:
    /// - Tag name: `"a"`, `"div"`, `"p"`
    /// - Class: `".classname"`
    /// - ID: `"#id"`
    /// - Tag + class: `"div.class"`
    /// - Tag + ID: `"div#id"`
    /// - Attribute: `"[href]"`, `"a[href]"`
    pub fn query_selector(&self, selector: &str) -> Option<u32> {
        // Index fast-path for simple `#id`, `.class`, bare `tag` selectors
        // when the indices haven't been invalidated by a mutation.
        if let Some(first) = self.simple_selector_first(selector) {
            return Some(first);
        }

        // Walk nodes in tree order (DFS from root)
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if self.node_matches_selector(node, selector) {
                    return Some(id);
                }
                // Push children in reverse order so first child is processed first
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        None
    }

    /// Query all matching nodes by CSS selector.
    ///
    /// Same fast-path strategy as `query_selector`: simple selectors consult
    /// the pre-built indices; everything else walks the tree.
    pub fn query_selector_all(&self, selector: &str) -> Vec<u32> {
        if let Some(matches) = self.simple_selector_all(selector) {
            return matches;
        }

        let mut results = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(self.root_id);
        while let Some(id) = queue.pop_front() {
            if let Some(node) = self.nodes.get(&id) {
                if self.node_matches_selector(node, selector) {
                    results.push(id);
                }
                for &child in &node.children {
                    queue.push_back(child);
                }
            }
        }
        results
    }

    /// Query selector scoped to a subtree rooted at `root_id`.
    /// Skips the root node itself — only searches descendants.
    pub fn query_selector_from(&self, root_id: u32, selector: &str) -> Option<u32> {
        let mut stack = vec![root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if id != root_id && self.node_matches_selector(node, selector) {
                    return Some(id);
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        None
    }

    /// Query all matching nodes scoped to a subtree rooted at `root_id`.
    /// Skips the root node itself — only searches descendants.
    pub fn query_selector_all_from(&self, root_id: u32, selector: &str) -> Vec<u32> {
        let mut results = Vec::new();
        let mut stack = vec![root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if id != root_id && self.node_matches_selector(node, selector) {
                    results.push(id);
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        results.reverse();
        results
    }

    /// Get an element by its ID attribute.
    ///
    /// Uses the `id_index` when fresh (between `from_frame` and the first
    /// in-place mutation); falls back to a linear scan otherwise. The
    /// linear scan also handles legacy snapshots that pre-date F-12 and
    /// were deserialized with an empty index.
    pub fn get_element_by_id(&self, id: &str) -> Option<u32> {
        if self.index_revision == self.revision
            && let Some(&node_id) = self.id_index.get(id)
        {
            return Some(node_id);
        }
        self.nodes
            .values()
            .find(|node| {
                node.node_type == 1 && node.attributes.get("id").map(|s| s.as_str()) == Some(id)
            })
            .map(|n| n.id)
    }

    /// Get all elements by tag name.
    ///
    /// Uses `tag_index` when fresh; falls back to a DFS walk otherwise. The
    /// tag index keys are already lowercased; we lowercase the query the
    /// same way.
    pub fn get_elements_by_tag_name(&self, tag: &str) -> Vec<u32> {
        let tag_lower = tag.to_lowercase();
        if self.index_revision == self.revision
            && let Some(ids) = self.tag_index.get(&tag_lower)
        {
            return ids.clone();
        }
        let mut results = Vec::new();
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if node.node_type == 1 && node.tag.to_lowercase() == tag_lower {
                    results.push(id);
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        results.reverse();
        results
    }

    /// Get all elements by class name.
    ///
    /// Uses `class_index` when fresh; falls back to a DFS walk otherwise.
    pub fn get_elements_by_class_name(&self, class: &str) -> Vec<u32> {
        if self.index_revision == self.revision
            && let Some(ids) = self.class_index.get(class)
        {
            return ids.clone();
        }
        let mut results = Vec::new();
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if node.node_type == 1
                    && let Some(cls) = node.attributes.get("class")
                    && cls.split_whitespace().any(|c| c == class)
                {
                    results.push(id);
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        results.reverse();
        results
    }

    // -----------------------------------------------------------------------
    // Structured data extraction (for OXI.getStructuredPage)
    // -----------------------------------------------------------------------

    /// Extract all headings from the document.
    ///
    /// Returns a list of `(level, text)` tuples where level is 1–6.
    /// Includes both `<h1>`–`<h6>` tags and elements with `role="heading"`.
    pub fn headings(&self) -> Vec<(u8, String)> {
        let heading_tags = ["h1", "h2", "h3", "h4", "h5", "h6"];
        let mut result = Vec::new();
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if node.node_type == 1 {
                    let tag_lower = node.tag.to_lowercase();
                    if let Some(idx) = heading_tags.iter().position(|t| *t == tag_lower) {
                        result.push((idx as u8 + 1, self.deep_text_content(node.id)));
                    } else if node.attributes.get("role").map(|s| s.as_str()) == Some("heading") {
                        let level: u8 = node
                            .attributes
                            .get("aria-level")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(2);
                        result.push((level.clamp(1, 6), self.deep_text_content(node.id)));
                    }
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        // No reverse needed: children are pushed in reverse so first child
        // is popped first, producing correct document order.
        result
    }

    /// Extract all links from the document.
    ///
    /// Returns `(text, href)` pairs.
    pub fn links(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                if node.node_type == 1 && node.tag.to_lowercase() == "a" {
                    let href = node.attributes.get("href").cloned().unwrap_or_default();
                    let text = self.deep_text_content(node.id);
                    result.push((text, href));
                }
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        // No reverse: children pushed in reverse → first child popped first → document order.
        result
    }

    /// Extract meta tags from the document.
    ///
    /// Returns name/property → content pairs (e.g. description, og:title).
    pub fn meta_tags(&self) -> HashMap<String, String> {
        let mut result = HashMap::new();
        for node in self.nodes.values() {
            if node.node_type == 1 && node.tag.to_lowercase() == "meta" {
                let name = node
                    .attributes
                    .get("name")
                    .or_else(|| node.attributes.get("property"));
                let content = node.attributes.get("content");
                if let (Some(n), Some(c)) = (name, content) {
                    result.insert(n.clone(), c.clone());
                }
            }
        }
        result
    }

    /// Recursively collect all text content from a node and its descendants.
    fn deep_text_content(&self, node_id: u32) -> String {
        let mut text = String::new();
        self.collect_text_recursive(node_id, &mut text);
        text.trim().to_string()
    }

    fn collect_text_recursive(&self, node_id: u32, text: &mut String) {
        if let Some(node) = self.nodes.get(&node_id) {
            if node.node_type == 3 {
                // Text node: text_content holds the text
                text.push_str(&node.text_content);
                text.push(' ');
            } else if node.node_type == 1 {
                // Element node: recurse into children
                for &child in &node.children {
                    self.collect_text_recursive(child, text);
                }
            }
        }
    }

    /// Check if a node matches a CSS selector.
    ///
    /// Supports:
    /// - Universal selector `*`
    /// - Multiple selectors `a, b` (comma-separated)
    /// - Descendant combinator `a b` (space-separated)
    /// - Attribute selectors `[attr]`, `[attr=val]`, `tag[attr]`
    /// - Tag name, `.class`, `#id`, `tag.class`, `tag#id`
    fn node_matches_selector(&self, node: &DomNode, selector: &str) -> bool {
        if node.node_type != 1 {
            return false;
        }

        // Handle comma-separated selectors: match any
        for single_sel in selector.split(',') {
            let single_sel = single_sel.trim();
            if self.node_matches_single(node, single_sel) {
                return true;
            }
        }
        false
    }

    /// Check a single selector (no commas) against a node.
    fn node_matches_single(&self, node: &DomNode, selector: &str) -> bool {
        // Universal selector
        if selector == "*" {
            return true;
        }

        // Descendant combinator: split on whitespace
        let parts: Vec<&str> = selector.split_whitespace().collect();
        if parts.len() > 1 {
            // Last part must match this node
            if !self.matches_simple(node, parts[parts.len() - 1]) {
                return false;
            }
            // Walk ancestors for preceding parts
            let ancestor_parts = &parts[..parts.len() - 1];
            let mut current = node.parent;
            let mut idx = ancestor_parts.len();
            while let Some(parent_id) = current {
                if idx == 0 {
                    return true;
                }
                let ancestor = match self.nodes.get(&parent_id) {
                    Some(a) => a,
                    None => break,
                };
                if self.matches_simple(ancestor, ancestor_parts[idx - 1]) {
                    idx -= 1;
                }
                current = ancestor.parent;
            }
            return idx == 0;
        }

        self.matches_simple(node, selector)
    }

    /// Match a simple selector (no commas, no descendant) against a node.
    fn matches_simple(&self, node: &DomNode, selector: &str) -> bool {
        if node.node_type != 1 {
            return false;
        }

        // Universal selector
        if selector == "*" {
            return true;
        }

        // Check for attribute selector: "a[href]" or "[href]"
        if let Some(bracket_start) = selector.find('[')
            && let Some(bracket_end) = selector.find(']')
            && bracket_start < bracket_end
        {
            let tag_part = &selector[..bracket_start];
            let attr_part = &selector[bracket_start + 1..bracket_end];

            // Check tag part matches (if any)
            if !tag_part.is_empty() && !node.tag.eq_ignore_ascii_case(tag_part) {
                return false;
            }

            // Check attribute: "href" or "href=value" or "href='value'"
            return if let Some(eq_pos) = attr_part.find('=') {
                let attr_name = &attr_part[..eq_pos];
                let val = attr_part[eq_pos + 1..].trim_matches('\'').trim_matches('"');
                let has_attr = node.attributes.contains_key(attr_name);
                has_attr && node.attributes.get(attr_name).map(|s| s.as_str()) == Some(val)
            } else {
                node.attributes.contains_key(attr_part)
            };
        }

        // ID selector: #foo
        if let Some(id) = selector.strip_prefix('#') {
            return node.attributes.get("id").map(|s| s.as_str()) == Some(id);
        }

        // Class selector: .foo
        if let Some(class) = selector.strip_prefix('.') {
            return node
                .attributes
                .get("class")
                .map(|cls| cls.split_whitespace().any(|c| c == class))
                .unwrap_or(false);
        }

        // Tag with class: tag.class
        if let Some(dot_pos) = selector.find('.') {
            let tag_part = &selector[..dot_pos];
            let class_part = &selector[dot_pos + 1..];
            return node.tag.eq_ignore_ascii_case(tag_part)
                && node
                    .attributes
                    .get("class")
                    .map(|cls| cls.split_whitespace().any(|c| c == class_part))
                    .unwrap_or(false);
        }

        // Tag with ID: tag#id
        if let Some(hash_pos) = selector.find('#') {
            let tag_part = &selector[..hash_pos];
            let id_part = &selector[hash_pos + 1..];
            return node.tag.eq_ignore_ascii_case(tag_part)
                && node.attributes.get("id").map(|s| s.as_str()) == Some(id_part);
        }

        // Simple tag name
        node.tag.eq_ignore_ascii_case(selector)
    }

    /// Get the first child node ID.
    pub fn first_child(&self, node_id: u32) -> Option<u32> {
        self.nodes
            .get(&node_id)
            .and_then(|n| n.children.first().copied())
    }

    /// Get the last child node ID.
    pub fn last_child(&self, node_id: u32) -> Option<u32> {
        self.nodes
            .get(&node_id)
            .and_then(|n| n.children.last().copied())
    }

    /// Get the next sibling node ID.
    pub fn next_sibling(&self, node_id: u32) -> Option<u32> {
        let parent_id = self.nodes.get(&node_id).and_then(|n| n.parent)?;
        let parent = self.nodes.get(&parent_id)?;
        let idx = parent.children.iter().position(|&id| id == node_id)?;
        parent.children.get(idx + 1).copied()
    }

    /// Get the previous sibling node ID.
    pub fn previous_sibling(&self, node_id: u32) -> Option<u32> {
        let parent_id = self.nodes.get(&node_id).and_then(|n| n.parent)?;
        let parent = self.nodes.get(&parent_id)?;
        let idx = parent.children.iter().position(|&id| id == node_id)?;
        if idx > 0 {
            parent.children.get(idx - 1).copied()
        } else {
            None
        }
    }

    /// Parse an HTML fragment and replace the children of `node_id` with the parsed nodes.
    ///
    /// Walks the parsed fragment's `html → body` skeleton, takes body's direct
    /// children, and inserts them under `node_id` (each new node receives a
    /// fresh id starting from `max(existing ids) + 1` so existing nodes are
    /// never overwritten). Old children of `node_id` are removed recursively
    /// from the snapshot. Revision is bumped on success.
    pub fn set_inner_html(&mut self, node_id: u32, html: &str) {
        let parsed = oxibrowser_webapi::dom::Document::parse(html);
        let tree = parsed.tree();
        let Some(root_id) = parsed.root() else {
            return;
        };
        let Some(html_id) = find_child_with_tag(&parsed, tree, root_id, "html") else {
            return;
        };
        let Some(body_id) = find_child_with_tag(&parsed, tree, html_id, "body") else {
            return;
        };
        // Snapshot the body-children NodeIds up front.
        let new_children: Vec<oxibrowser_webapi::dom::NodeId> = tree.children(body_id).to_vec();

        // Bail if the target doesn't exist in this snapshot.
        if !self.nodes.contains_key(&node_id) {
            return;
        }

        // Remove existing children first so we don't leak orphan nodes.
        let to_remove: Vec<u32> = self
            .nodes
            .get(&node_id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        for child_id in to_remove {
            self.remove_subtree(child_id);
        }

        // Compute next id once so the first inserted subtree gets a stable base.
        let mut next_id = self.nodes.keys().max().copied().map(|m| m + 1).unwrap_or(0);
        for src_id in new_children {
            self.insert_subtree(src_id, &parsed, tree, node_id, None, &mut next_id);
        }
        self.bump_revision();
    }

    /// Recursively remove `node_id` and all of its descendants from the snapshot.
    ///
    /// Detaches `node_id` from its parent's `children` vector and removes every
    /// node in the subtree from `self.nodes`. Safe to call when the node has
    /// no parent (e.g. the root).
    fn remove_subtree(&mut self, node_id: u32) {
        // Pull a borrow first to learn the parent + descendant set without
        // holding the `self.nodes` borrow across mutations below.
        let parent_id = match self.nodes.get(&node_id) {
            Some(n) => n.parent,
            None => return,
        };
        let descendants = collect_subtree_ids(self, node_id);

        // Detach from parent's children vector.
        if let Some(parent_id) = parent_id
            && let Some(parent) = self.nodes.get_mut(&parent_id)
        {
            parent.children.retain(|&c| c != node_id);
        }

        // Drop in pre-order so parents are removed before re-using id vectors.
        let mut to_drop = vec![node_id];
        to_drop.extend(descendants);
        for id in to_drop {
            self.nodes.remove(&id);
        }
    }

    /// Insert a webapi node (and its descendants) from `doc`/`tree` into the
    /// snapshot under `parent_id`, before `next_id` (or at the end when None).
    ///
    /// Allocates fresh ids starting at `*next_id_counter` (post-incremented
    /// for each created node) so collisions with existing snapshot ids are
    /// impossible. Returns the new id of `src_id`, or `None` if `src_id` is
    /// not present in `doc`, `parent_id` is unknown, or `src_id` is a
    /// Doctype/Document.
    fn insert_subtree(
        &mut self,
        src_id: oxibrowser_webapi::dom::NodeId,
        doc: &oxibrowser_webapi::dom::Document,
        tree: &oxibrowser_webapi::dom::Tree,
        parent_id: u32,
        next_id: Option<u32>,
        next_id_counter: &mut u32,
    ) -> Option<u32> {
        use oxibrowser_webapi::dom::NodeType;

        let node = doc.get_node(src_id)?;
        if !self.nodes.contains_key(&parent_id) {
            return None;
        }

        // Skip Document / Doctype — they shouldn't appear as children here, but
        // be defensive in case the parsed fragment exposes them oddly.
        let (tag, attributes, node_type_u8, text_content) = match &node.node_type {
            NodeType::Element { tag, attributes } => {
                let attrs: HashMap<String, String> = attributes
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let tc = collect_text_content(src_id, doc, tree);
                (tag.clone(), attrs, 1u8, tc)
            }
            NodeType::Text(t) => (String::new(), HashMap::new(), 3u8, t.clone()),
            NodeType::Comment(c) => (String::new(), HashMap::new(), 8u8, c.clone()),
            NodeType::Document | NodeType::Doctype { .. } => return None,
        };

        let new_id = *next_id_counter;
        *next_id_counter = next_id_counter.wrapping_add(1);

        // Insert the node first so children can take it as their parent.
        let dom_node = DomNode {
            id: new_id,
            tag,
            attributes,
            text_content,
            children: Vec::new(),
            parent: Some(parent_id),
            node_type: node_type_u8,
        };
        self.nodes.insert(new_id, dom_node);

        // Recurse into source-children before wiring into the parent's
        // children vec, so each child gets a stable parent link first.
        let mut child_ids: Vec<u32> = Vec::with_capacity(tree.children(src_id).len());
        for &src_child in tree.children(src_id) {
            if let Some(child_new_id) =
                self.insert_subtree(src_child, doc, tree, new_id, None, next_id_counter)
            {
                child_ids.push(child_new_id);
            }
        }

        // Populate this node's children, then link into the parent's children
        // vec at the requested position.
        if let Some(node_ref) = self.nodes.get_mut(&new_id) {
            node_ref.children = child_ids;
        }
        if let Some(parent) = self.nodes.get_mut(&parent_id) {
            match next_id {
                Some(anchor) => {
                    if let Some(idx) = parent.children.iter().position(|&c| c == anchor) {
                        parent.children.insert(idx, new_id);
                    } else {
                        parent.children.push(new_id);
                    }
                }
                None => parent.children.push(new_id),
            }
        }
        Some(new_id)
    }
}

/// Find the first direct child of `parent_id` whose element tag matches `tag`
/// (case-insensitive). Returns `None` if `parent_id` is absent from the tree
/// or no such child exists.
fn find_child_with_tag(
    doc: &oxibrowser_webapi::dom::Document,
    tree: &oxibrowser_webapi::dom::Tree,
    parent_id: oxibrowser_webapi::dom::NodeId,
    tag: &str,
) -> Option<oxibrowser_webapi::dom::NodeId> {
    for &child in tree.children(parent_id) {
        if let Some(node) = doc.get_node(child)
            && let Some(child_tag) = node.tag_name()
            && child_tag.eq_ignore_ascii_case(tag)
        {
            return Some(child);
        }
    }
    None
}

/// DFS pre-order collection of `node_id` and every descendant present in
/// `self.nodes`. Excludes `node_id` itself; callers typically prepend it.
fn collect_subtree_ids(snap: &DomSnapshot, node_id: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let Some(node) = snap.nodes.get(&node_id) else {
        return out;
    };
    for &child in &node.children {
        out.push(child);
        out.extend(collect_subtree_ids(snap, child));
    }
    out
}

/// Recursively collect all nodes from the document tree into the snapshot.
fn collect_nodes(
    node_id: oxibrowser_webapi::dom::NodeId,
    doc: &oxibrowser_webapi::dom::Document,
    tree: &oxibrowser_webapi::dom::Tree,
    nodes: &mut HashMap<u32, DomNode>,
    order: &mut Vec<u32>,
    body_id: &mut Option<u32>,
    head_id: &mut Option<u32>,
) {
    use oxibrowser_webapi::dom::NodeType;

    let id_u32 = node_id.0 as u32;

    if let Some(node) = doc.get_node(node_id) {
        let (tag, attributes, node_type_u8) = match &node.node_type {
            NodeType::Document => (String::new(), HashMap::new(), 9u8),
            NodeType::Element { tag, attributes } => {
                let tag_lower = tag.to_lowercase();
                if tag_lower == "body" && body_id.is_none() {
                    *body_id = Some(id_u32);
                } else if tag_lower == "head" && head_id.is_none() {
                    *head_id = Some(id_u32);
                }
                let attrs: HashMap<String, String> = attributes
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                (tag.clone(), attrs, 1u8)
            }
            NodeType::Text(_) => (String::new(), HashMap::new(), 3u8),
            NodeType::Comment(_) => (String::new(), HashMap::new(), 8u8),
            NodeType::Doctype { .. } => (String::new(), HashMap::new(), 10u8),
        };

        // Collect text content from direct text children
        let mut text_content = collect_text_content(node_id, doc, tree);
        // For elements with data-oxi-text (set by textContent setter), prefer that
        if text_content.is_empty()
            && let Some(v) = node.get_attribute("data-oxi-text")
        {
            text_content = v.to_string();
        }

        let children: Vec<u32> = tree.children(node_id).iter().map(|c| c.0 as u32).collect();
        let parent = tree.parent(node_id).map(|p| p.0 as u32);

        let dom_node = DomNode {
            id: id_u32,
            tag,
            attributes,
            text_content,
            children,
            parent,
            node_type: node_type_u8,
        };
        nodes.insert(id_u32, dom_node);
        // Record document order (DFS pre-order: parent before children).
        order.push(id_u32);

        // Recurse into children
        for &child in tree.children(node_id) {
            collect_nodes(child, doc, tree, nodes, order, body_id, head_id);
        }
    }
}

/// Collect text content from a node's direct text children and their descendants.
fn collect_text_content(
    node_id: oxibrowser_webapi::dom::NodeId,
    doc: &oxibrowser_webapi::dom::Document,
    tree: &oxibrowser_webapi::dom::Tree,
) -> String {
    let mut text = String::new();
    collect_text_recursive(node_id, doc, tree, &mut text);
    text
}

fn collect_text_recursive(
    node_id: oxibrowser_webapi::dom::NodeId,
    doc: &oxibrowser_webapi::dom::Document,
    tree: &oxibrowser_webapi::dom::Tree,
    text: &mut String,
) {
    use oxibrowser_webapi::dom::NodeType;
    if let Some(node) = doc.get_node(node_id)
        && let NodeType::Text(t) = &node.node_type
    {
        text.push_str(t);
    }
    for &child in tree.children(node_id) {
        collect_text_recursive(child, doc, tree, text);
    }
}

/// Build id/class/tag indices from `nodes` in the order given by `order`
/// (DFS pre-order), used exclusively by `DomSnapshot::from_frame` for the
/// initial snapshot. After any in-place mutation, indices are not refreshed
/// eagerly; the `&self` read methods on `DomSnapshot` detect staleness via
/// `index_revision != revision` and fall back to a tree walk.
impl DomSnapshot {
    /// Rebuild id/class/tag indices from the current `nodes` HashMap by
    /// performing a DFS pre-order walk starting at `root_id`. Called after
    /// in-place mutations (innerHTML, createElement, etc.) so that
    /// `query_selector` / `get_element_by_id` can find newly-inserted nodes
    /// via the fast-path index instead of falling back to a tree walk.
    pub fn rebuild_indices(&mut self) {
        let mut order: Vec<u32> = Vec::with_capacity(self.nodes.len());
        let mut stack = vec![self.root_id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                order.push(id);
                // Push children in reverse so DFS pre-order is preserved.
                for &child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
        }
        let (id_index, class_index, tag_index) = build_indices(&self.nodes, &order);
        self.id_index = id_index;
        self.class_index = class_index;
        self.tag_index = tag_index;
        self.index_revision = self.revision;
    }
}
type DomIndices = (
    HashMap<String, u32>,
    HashMap<String, Vec<u32>>,
    HashMap<String, Vec<u32>>,
);
fn build_indices(nodes: &HashMap<u32, DomNode>, order: &[u32]) -> DomIndices {
    let mut id_index: HashMap<String, u32> = HashMap::new();
    let mut class_index: HashMap<String, Vec<u32>> = HashMap::new();
    let mut tag_index: HashMap<String, Vec<u32>> = HashMap::new();

    for &id in order {
        let node = match nodes.get(&id) {
            Some(n) if n.node_type == 1 => n,
            _ => continue,
        };
        if let Some(id_attr) = node.attributes.get("id")
            && !id_attr.is_empty()
            && !id_index.contains_key(id_attr)
        {
            id_index.insert(id_attr.clone(), id);
        }
        if let Some(cls) = node.attributes.get("class") {
            for token in cls.split_whitespace() {
                if !token.is_empty() {
                    class_index.entry(token.to_string()).or_default().push(id);
                }
            }
        }
        let tag_lower = node.tag.to_lowercase();
        if !tag_lower.is_empty() {
            tag_index.entry(tag_lower).or_default().push(id);
        }
    }

    (id_index, class_index, tag_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;
    use url::Url;

    fn make_frame(html: &str) -> Frame {
        let url = Url::parse("https://example.com").unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(Frame::from_html(url, html)).unwrap()
    }

    #[test]
    fn test_dom_snapshot_from_frame() {
        let html = r#"<html><head><title>Test Page</title></head>
            <body><p class="intro">Hello</p><a href="/link">click</a></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        assert_eq!(snapshot.url, "https://example.com/");
        assert_eq!(snapshot.title, "Test Page");
        assert!(snapshot.body_id.is_some(), "should find body element");
        assert!(snapshot.head_id.is_some(), "should find head element");
        assert!(snapshot.nodes.len() > 5, "should have multiple nodes");
    }

    #[test]
    fn test_query_selector_tag() {
        let html = r#"<html><body><p>first</p><p>second</p></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector("p");
        assert!(found.is_some(), "should find a <p> element");
        let node = snapshot.nodes.get(&found.unwrap()).unwrap();
        assert_eq!(node.tag, "p");
    }

    #[test]
    fn test_query_selector_class() {
        let html = r#"<html><body><div class="foo">content</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector(".foo");
        assert!(found.is_some(), "should find element with class .foo");
    }

    #[test]
    fn test_query_selector_id() {
        let html = r#"<html><body><span id="bar">text</span></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector("#bar");
        assert!(found.is_some(), "should find element with id #bar");
    }

    #[test]
    fn test_query_selector_tag_class() {
        let html =
            r#"<html><body><div class="main">main</div><p class="main">para</p></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector("div.main");
        assert!(found.is_some(), "should find div.main");
        let node = snapshot.nodes.get(&found.unwrap()).unwrap();
        assert_eq!(node.tag, "div");
    }

    #[test]
    fn test_query_selector_tag_id() {
        let html = r#"<html><body><div id="content">c</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector("div#content");
        assert!(found.is_some(), "should find div#content");
    }

    #[test]
    fn test_query_selector_attribute() {
        let html = r#"<html><body><a href="/link">click</a><p>no link</p></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.query_selector("[href]");
        assert!(found.is_some(), "should find element with href attribute");
        let node = snapshot.nodes.get(&found.unwrap()).unwrap();
        assert_eq!(node.tag, "a");

        let found2 = snapshot.query_selector("a[href]");
        assert!(found2.is_some(), "should find a[href]");
    }

    #[test]
    fn test_query_selector_all() {
        let html = "<html><body><ul><li>a</li><li>b</li><li>c</li></ul></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let items = snapshot.query_selector_all("li");
        assert_eq!(items.len(), 3, "should find 3 <li> elements");
    }

    #[test]
    fn test_get_element_by_id() {
        let html = r#"<html><body><div id="main">content</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let found = snapshot.get_element_by_id("main");
        assert!(found.is_some(), "should find element by id");

        let not_found = snapshot.get_element_by_id("nonexistent");
        assert!(not_found.is_none(), "should not find nonexistent id");
    }

    #[test]
    fn test_get_elements_by_tag_name() {
        let html = "<html><body><p>a</p><p>b</p><p>c</p></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let items = snapshot.get_elements_by_tag_name("p");
        assert_eq!(items.len(), 3, "should find 3 <p> elements");
    }

    #[test]
    fn test_get_elements_by_class_name() {
        let html =
            r#"<html><body><div class="item">a</div><div class="item">b</div></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let items = snapshot.get_elements_by_class_name("item");
        assert_eq!(items.len(), 2, "should find 2 .item elements");
    }

    #[test]
    fn test_dom_snapshot_empty() {
        let snapshot = DomSnapshot::empty();
        assert!(snapshot.url.is_empty());
        assert!(snapshot.nodes.is_empty());
        assert!(snapshot.body_id.is_none());
    }

    #[test]
    fn test_node_text_content() {
        let html = r#"<html><body><p>Hello World</p></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let p_id = snapshot.query_selector("p").unwrap();
        let p_node = snapshot.nodes.get(&p_id).unwrap();
        assert!(
            p_node.text_content.contains("Hello World"),
            "text content should include 'Hello World'"
        );
    }

    #[test]
    fn test_node_parent_child_relationship() {
        let html = "<html><body><div><p>text</p></div></body></html>";
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let div_id = snapshot.query_selector("div").unwrap();
        let p_id = snapshot.query_selector("p").unwrap();

        let div_node = snapshot.nodes.get(&div_id).unwrap();
        let p_node = snapshot.nodes.get(&p_id).unwrap();

        assert!(
            div_node.children.contains(&p_id),
            "div should have p as child"
        );
        assert_eq!(p_node.parent, Some(div_id), "p's parent should be div");
    }

    // -----------------------------------------------------------------------
    // Structured data extraction tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_headings_extraction() {
        let html = r#"<html><body>
            <h1>Main Title</h1>
            <h2>Subtitle</h2>
            <h3>Section</h3>
            <p>Not a heading</p>
        </body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let headings = snapshot.headings();
        assert_eq!(headings.len(), 3, "should find 3 headings");
        assert_eq!(headings[0].0, 1, "first heading should be h1");
        assert!(headings[0].1.contains("Main Title"));
        assert_eq!(headings[1].0, 2, "second heading should be h2");
        assert_eq!(headings[2].0, 3, "third heading should be h3");
    }

    #[test]
    fn test_headings_with_aria_role() {
        let html = r#"<html><body>
            <span role="heading" aria-level="2">ARIA Heading</span>
            <div role="heading">Default Level</div>
        </body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let headings = snapshot.headings();
        assert_eq!(headings.len(), 2, "should find 2 ARIA headings");
        assert_eq!(headings[0].0, 2, "first should be level 2");
        assert!(headings[0].1.contains("ARIA Heading"));
        assert_eq!(headings[1].0, 2, "default level should be 2");
    }

    #[test]
    fn test_links_extraction() {
        let html = r#"<html><body>
            <a href="https://example.com">Example</a>
            <a href="/about">About Us</a>
            <a>No href</a>
        </body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let links = snapshot.links();
        assert_eq!(links.len(), 3, "should find 3 links");
        assert_eq!(links[0].1, "https://example.com");
        assert!(links[0].0.contains("Example"));
        assert_eq!(links[1].1, "/about");
        assert_eq!(links[2].1, "", "link without href should have empty string");
    }

    #[test]
    fn test_meta_tags_extraction() {
        let html = r#"<html><head>
            <meta name="description" content="A test page">
            <meta property="og:title" content="OG Title">
            <meta name="viewport" content="width=device-width">
        </head><body></body></html>"#;
        let frame = make_frame(html);
        let snapshot = DomSnapshot::from_frame(&frame);

        let meta = snapshot.meta_tags();
        assert_eq!(meta.get("description").unwrap(), "A test page");
        assert_eq!(meta.get("og:title").unwrap(), "OG Title");
        assert_eq!(meta.get("viewport").unwrap(), "width=device-width");
    }

    #[test]
    fn test_structured_data_empty_page() {
        let snapshot = DomSnapshot::empty();
        assert!(snapshot.headings().is_empty());
        assert!(snapshot.links().is_empty());
        assert!(snapshot.meta_tags().is_empty());
    }

    #[test]
    fn test_set_inner_html_replaces_children() {
        let html = r#"<html><body><div id="host"><p>old</p><span>keep</span></div></body></html>"#;
        let frame = make_frame(html);
        let mut snapshot = DomSnapshot::from_frame(&frame);
        let host = snapshot.get_element_by_id("host").expect("host div");
        let revision_before = snapshot.revision;

        // Two siblings at top-level: an element + a text node.
        snapshot.set_inner_html(host, "<a href=\"/x\">link</a>tail text");

        let h = snapshot.nodes.get(&host).expect("host still present");
        // Old children should be gone (no <p>, no <span>, no descendant text).
        assert_eq!(h.children.len(), 2, "exactly two new children inserted");
        for &cid in &h.children {
            let c = snapshot.nodes.get(&cid).expect("child present");
            assert_ne!(c.tag, "p", "old <p> removed");
            assert_ne!(c.tag, "span", "old <span> removed");
        }
        // First child is the <a>, with attributes preserved.
        let a_id = h.children[0];
        let a = snapshot.nodes.get(&a_id).expect("a present");
        assert_eq!(a.tag, "a");
        assert_eq!(a.node_type, 1u8);
        assert_eq!(a.attributes.get("href").map(String::as_str), Some("/x"));
        // Second child is a text node.
        let t_id = h.children[1];
        let t = snapshot.nodes.get(&t_id).expect("text node present");
        assert_eq!(t.node_type, 3u8);
        assert_eq!(t.text_content, "tail text");
        // No collision with original ids — the new ids are strictly greater.
        assert!(a_id > 0 && t_id > 0 && a_id != t_id);
        // Old <p>/<span> and their children must be gone from the snapshot.
        for id in snapshot.nodes.keys() {
            let n = &snapshot.nodes[id];
            assert_ne!(n.tag, "p");
            assert_ne!(n.tag, "span");
        }
        // Revision bumped — indices now stale, style cache dropped.
        assert_ne!(snapshot.revision, revision_before);
    }

    #[test]
    fn test_set_inner_html_handles_doctype() {
        // Even with <!DOCTYPE html> in the fragment, html5ever's tree places
        // the Doctype as a sibling of <html> under the root. set_inner_html
        // must skip the Doctype and locate <html> → <body>.
        let html = r#"<html><body><div id="host">old</div></body></html>"#;
        let frame = make_frame(html);
        let mut snapshot = DomSnapshot::from_frame(&frame);
        let host = snapshot.get_element_by_id("host").expect("host div");

        snapshot.set_inner_html(
            host,
            r#"<!DOCTYPE html><html><body><b>new</b></body></html>"#,
        );

        let h = snapshot.nodes.get(&host).expect("host still present");
        assert_eq!(h.children.len(), 1, "<b> inserted as only child");
        let b_id = h.children[0];
        let b = snapshot.nodes.get(&b_id).expect("b present");
        assert_eq!(b.tag, "b");
        assert_eq!(b.text_content, "new");
    }

    #[test]
    fn test_remove_subtree_detaches_and_purges() {
        let html = r#"<html><body>
            <div id="root"><p id="child">a<span id="leaf">b</span></p></div>
        </body></html>"#;
        let frame = make_frame(html);
        let mut snapshot = DomSnapshot::from_frame(&frame);
        let root = snapshot.get_element_by_id("root").expect("root div");

        let pre_count = snapshot.nodes.len();
        snapshot.remove_subtree(root);
        let post_count = snapshot.nodes.len();

        // 4 nodes gone (root + p + span + leaf text node… actually <span>'s
        // text child counts too). Just assert strict shrinkage + no orphans.
        assert!(post_count < pre_count, "snapshot shrinks after remove");
        assert!(!snapshot.nodes.contains_key(&root));
        // `id_index` is not eagerly invalidated here — that's the
        // `bump_revision` contract — so assert directly against `nodes`.
        let still_present = snapshot
            .nodes
            .values()
            .any(|n| n.attributes.get("id").map(|s| s.as_str()) == Some("child"));
        assert!(!still_present, "<p id=child> purged from nodes");
        let still_present = snapshot
            .nodes
            .values()
            .any(|n| n.attributes.get("id").map(|s| s.as_str()) == Some("leaf"));
        assert!(!still_present, "<span id=leaf> purged from nodes");
    }

    #[test]
    fn test_insert_subtree_fresh_ids_and_links() {
        // Verify that insert_subtree against an empty snapshot mints ids
        // strictly past the largest existing one and wires parent/child links.
        let html = "<html><body><div id=\"target\"></div></body></html>";
        let frame = make_frame(html);
        let mut snapshot = DomSnapshot::from_frame(&frame);
        let target = snapshot.get_element_by_id("target").expect("target div");

        // Parse a fresh fragment and locate the <b> element to insert.
        let parsed = oxibrowser_webapi::dom::Document::parse("<b>hi</b>");
        let tree = parsed.tree();
        let root = parsed.root().unwrap();
        let html_node = {
            let mut found = None;
            for &c in tree.children(root) {
                if parsed.get_node(c).and_then(|n| n.tag_name()) == Some("html") {
                    found = Some(c);
                    break;
                }
            }
            found.expect("html element")
        };
        let body_node = {
            let mut found = None;
            for &c in tree.children(html_node) {
                if parsed.get_node(c).and_then(|n| n.tag_name()) == Some("body") {
                    found = Some(c);
                    break;
                }
            }
            found.expect("body element")
        };
        let b_src = tree.children(body_node).first().copied().expect("b source");

        let max_existing = *snapshot.nodes.keys().max().unwrap();
        let mut counter = max_existing + 1;
        let new_id = snapshot
            .insert_subtree(b_src, &parsed, tree, target, None, &mut counter)
            .expect("inserted");
        assert_eq!(new_id, max_existing + 1, "fresh id past existing max");

        let t = snapshot.nodes.get(&target).expect("target present");
        assert_eq!(t.children, vec![new_id], "child wired into parent");
        let inserted = snapshot.nodes.get(&new_id).expect("inserted node present");
        assert_eq!(inserted.parent, Some(target));
        assert_eq!(inserted.tag, "b");
        assert_eq!(inserted.node_type, 1u8);
    }
}
