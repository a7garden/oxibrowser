//! HTML DOM serializer.
//!
//! Walks a `DomSnapshot` node tree and emits an HTML string. Used by the
//! `innerHTML` / `outerHTML` getters exposed on JS element objects.
//!
//! Output is deterministic (attribute keys are sorted) and HTML-safe
//! (`&`, `<`, `>`, `"` are escaped per context).

use crate::js::dom_snapshot::{DomNode, DomSnapshot};

/// HTML5 void elements — self-closing tags that never receive a closing tag
/// or element children.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Serialize a single DOM node to HTML.
///
/// For an Element this emits the opening tag, attributes, children, and
/// closing tag. For a Text node the content is escaped. For a Comment
/// the `<!-- ... -->` wrapper is emitted. For Document / other container
/// types only the children are emitted (matches browser `outerHTML`
/// semantics for `Document`).
pub fn serialize_node(node: &DomNode, snapshot: &DomSnapshot, buf: &mut String) {
    match node.node_type {
        1 => serialize_element(node, snapshot, buf),
        3 => {
            // Text node — escape `&`, `<`, `>` so the output is parseable HTML.
            let escaped = escape_text(&node.text_content);
            buf.push_str(&escaped);
        }
        8 => {
            buf.push_str("<!--");
            buf.push_str(&node.text_content);
            buf.push_str("-->");
        }
        _ => {
            // Document (9) or any other container type — serialize children only.
            serialize_children(node, snapshot, buf);
        }
    }
}

/// Serialize the children of a node, not the node itself. Used by the
/// `innerHTML` getter.
pub fn serialize_children(node: &DomNode, snapshot: &DomSnapshot, buf: &mut String) {
    for &child_id in &node.children {
        if let Some(child) = snapshot.nodes.get(&child_id) {
            serialize_node(child, snapshot, buf);
        }
    }
}

fn serialize_element(node: &DomNode, snapshot: &DomSnapshot, buf: &mut String) {
    let tag = &node.tag;
    buf.push('<');
    buf.push_str(tag);

    // Sort attribute keys for deterministic output (HashMap iteration order
    // is otherwise unspecified and would round-trip inconsistently).
    let mut attrs: Vec<(&str, &str)> = node
        .attributes
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    attrs.sort_by(|a, b| a.0.cmp(b.0));

    for (name, value) in &attrs {
        buf.push(' ');
        buf.push_str(name);
        // Empty attribute value is serialized as just the bare name
        // (e.g. `<input disabled>`) — the same shortcut browsers take.
        if !value.is_empty() {
            buf.push_str("=\"");
            buf.push_str(&escape_attr(value));
            buf.push('"');
        }
    }

    if VOID_ELEMENTS.contains(&tag.as_str()) {
        // No closing tag and no children for void elements.
        buf.push('>');
        return;
    }

    buf.push('>');
    serialize_children(node, snapshot, buf);
    buf.push_str("</");
    buf.push_str(tag);
    buf.push('>');
}

/// Escape characters that would otherwise break out of a text node.
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape characters that would otherwise break out of a double-quoted
/// attribute value. (`'` is left alone — we always use `"` as the quote.)
/// `>` is escaped for round-trip safety even though the HTML5 spec only
/// mandates escaping the quote character used.
fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_elem(
        id: u32,
        tag: &str,
        attrs: Vec<(&str, &str)>,
        children: Vec<u32>,
        parent: Option<u32>,
    ) -> DomNode {
        DomNode {
            id,
            tag: tag.to_string(),
            attributes: attrs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            text_content: String::new(),
            children,
            parent,
            node_type: 1,
        }
    }

    fn make_text(id: u32, text: &str, parent: Option<u32>) -> DomNode {
        DomNode {
            id,
            tag: String::new(),
            attributes: HashMap::new(),
            text_content: text.to_string(),
            children: vec![],
            parent,
            node_type: 3,
        }
    }

    fn make_comment(id: u32, text: &str, parent: Option<u32>) -> DomNode {
        DomNode {
            id,
            tag: String::new(),
            attributes: HashMap::new(),
            text_content: text.to_string(),
            children: vec![],
            parent,
            node_type: 8,
        }
    }
    fn empty_snapshot(nodes: HashMap<u32, DomNode>, root_id: u32) -> DomSnapshot {
        let mut s = DomSnapshot::empty();
        s.url = "about:blank".into();
        s.nodes = nodes;
        s.root_id = root_id;
        s
    }

    #[test]
    fn single_element_with_attrs_and_text() {
        let nodes = HashMap::from([
            (
                1,
                make_elem(
                    1,
                    "div",
                    vec![("class", "foo"), ("id", "bar")],
                    vec![2],
                    None,
                ),
            ),
            (2, make_text(2, "hello", Some(1))),
        ]);
        let snap = empty_snapshot(nodes, 1);
        let mut buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut buf);
        assert_eq!(buf, r#"<div class="foo" id="bar">hello</div>"#);
    }

    #[test]
    fn attribute_keys_sorted_for_determinism() {
        // HashMap iteration order is unspecified; the serializer must sort
        // to make round-trips reproducible.
        let nodes = HashMap::from([(
            1,
            make_elem(
                1,
                "input",
                vec![("type", "text"), ("name", "q"), ("id", "x")],
                vec![],
                None,
            ),
        )]);
        let snap = empty_snapshot(nodes, 1);
        let mut buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut buf);
        assert_eq!(buf, r#"<input id="x" name="q" type="text">"#);
    }

    #[test]
    fn empty_attribute_emits_bare_name() {
        // `disabled` / `checked` / etc. carry no value in real HTML.
        let nodes = HashMap::from([(
            1,
            make_elem(1, "input", vec![("disabled", "")], vec![], None),
        )]);
        let snap = empty_snapshot(nodes, 1);
        let mut buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut buf);
        assert_eq!(buf, "<input disabled>");
    }

    #[test]
    fn void_element_self_closes() {
        let nodes = HashMap::from([(1, make_elem(1, "br", vec![], vec![], None))]);
        let snap = empty_snapshot(nodes, 1);
        let mut buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut buf);
        assert_eq!(buf, "<br>");
    }

    #[test]
    fn void_element_ignores_children_if_any() {
        // Even if a void element somehow has a child listed, the serializer
        // must not emit an end tag.
        let nodes = HashMap::from([
            (1, make_elem(1, "img", vec![], vec![2], None)),
            (2, make_text(2, "ignored", Some(1))),
        ]);
        let snap = empty_snapshot(nodes, 1);
        let mut buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut buf);
        assert_eq!(buf, "<img>");
    }

    #[test]
    fn text_node_escapes_specials() {
        let nodes = HashMap::from([(1, make_text(1, "a<>&b", None))]);
        let snap = empty_snapshot(nodes, 1);
        let mut buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut buf);
        assert_eq!(buf, "a&lt;&gt;&amp;b");
    }

    #[test]
    fn attribute_value_escapes_quotes_and_specials() {
        let nodes = HashMap::from([(
            1,
            make_elem(
                1,
                "a",
                vec![("href", "https://x?a=1&b=\"2\"<>")],
                vec![],
                None,
            ),
        )]);
        let snap = empty_snapshot(nodes, 1);
        let mut buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut buf);
        assert_eq!(
            buf,
            r#"<a href="https://x?a=1&amp;b=&quot;2&quot;&lt;&gt;"></a>"#
        );
    }

    #[test]
    fn comment_node_wraps_correctly() {
        let nodes = HashMap::from([(1, make_comment(1, " hello ", None))]);
        let snap = empty_snapshot(nodes, 1);
        let mut buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut buf);
        assert_eq!(buf, "<!-- hello -->");
    }

    #[test]
    fn document_node_emits_only_children() {
        // Document (type 9) — outerHTML semantics: skip the document wrapper.
        // The doc's children are emitted directly, no implicit <html> tag.
        let mut doc = make_text(1, "", None);
        doc.node_type = 9;
        doc.children = vec![2];
        let body = make_elem(2, "body", vec![], vec![3], Some(1));
        let hi = make_text(3, "hi", Some(2));
        let nodes = HashMap::from([(1, doc), (2, body), (3, hi)]);
        let snap = empty_snapshot(nodes, 1);
        let mut buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut buf);
        assert_eq!(buf, "<body>hi</body>");
    }

    #[test]
    fn serialize_children_skips_the_node_itself() {
        // innerHTML should never include the wrapper — verify by serializing
        // children only and comparing to the full node minus its opening/closing.
        let nodes = HashMap::from([
            (
                1,
                make_elem(1, "div", vec![("class", "x")], vec![2, 3], None),
            ),
            (2, make_text(2, "a", Some(1))),
            (3, make_elem(3, "span", vec![], vec![], Some(1))),
        ]);
        let snap = empty_snapshot(nodes, 1);

        let mut children_buf = String::new();
        serialize_children(snap.nodes.get(&1).unwrap(), &snap, &mut children_buf);
        assert_eq!(children_buf, "a<span></span>");

        let mut full_buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut full_buf);
        // full = outerHTML = wrapping tag + children
        assert_eq!(full_buf, r#"<div class="x">a<span></span></div>"#);
    }

    #[test]
    fn unknown_node_type_falls_through_to_children() {
        // Defensive: any unrecognized node_type behaves like a Document.
        let nodes = HashMap::from([
            (1, {
                let mut n = make_elem(1, "p", vec![], vec![2], None);
                n.node_type = 99; // unknown
                n
            }),
            (2, make_text(2, "inside", Some(1))),
        ]);
        let snap = empty_snapshot(nodes, 1);
        let mut buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut buf);
        assert_eq!(buf, "inside");
    }

    #[test]
    fn missing_child_id_is_silently_skipped() {
        // If a parent lists a child_id with no matching node, don't crash —
        // mirror the snapshot's own tolerance for dangling references.
        let nodes = HashMap::from([(1, {
            let mut n = make_elem(1, "div", vec![], vec![2, 999], None);
            n.node_type = 1;
            n
        })]);
        let snap = empty_snapshot(nodes, 1);
        let mut buf = String::new();
        serialize_node(snap.nodes.get(&1).unwrap(), &snap, &mut buf);
        // 999 doesn't exist — we just emit the opening/closing tags.
        assert_eq!(buf, "<div></div>");
    }
}
