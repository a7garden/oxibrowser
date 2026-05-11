//! Node types for the DOM tree.

/// Unique identifier for a DOM node within its document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub usize);

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "node-{}", self.0)
    }
}

/// Type of a DOM node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeType {
    /// Document node (root).
    Document,
    /// Element node with tag name.
    Element {
        tag: String,
        attributes: Vec<(String, String)>,
    },
    /// Text node.
    Text(String),
    /// Comment node.
    Comment(String),
    /// DOCTYPE node.
    Doctype {
        name: String,
    },
}

/// Data associated with a DOM node.
#[derive(Debug, Clone)]
pub struct Node {
    /// Unique ID within the document.
    pub id: NodeId,
    /// Node type and associated data.
    pub node_type: NodeType,
}

impl Node {
    /// Create a new node with the given ID and type.
    pub fn new(id: NodeId, node_type: NodeType) -> Self {
        Self { id, node_type }
    }

    /// Check if this is an element node.
    pub fn is_element(&self) -> bool {
        matches!(self.node_type, NodeType::Element { .. })
    }

    /// Check if this is a text node.
    pub fn is_text(&self) -> bool {
        matches!(self.node_type, NodeType::Text(_))
    }

    /// Get the tag name if this is an element.
    pub fn tag_name(&self) -> Option<&str> {
        match &self.node_type {
            NodeType::Element { tag, .. } => Some(tag),
            _ => None,
        }
    }

    /// Get the text content if this is a text node.
    pub fn text_content(&self) -> Option<&str> {
        match &self.node_type {
            NodeType::Text(text) => Some(text),
            _ => None,
        }
    }

    /// Get an attribute value by name.
    pub fn get_attribute(&self, name: &str) -> Option<&str> {
        match &self.node_type {
            NodeType::Element { attributes, .. } => attributes
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    /// Get the ID attribute.
    pub fn id_attr(&self) -> Option<&str> {
        self.get_attribute("id")
    }

    /// Get the class attribute.
    pub fn class_attr(&self) -> Option<&str> {
        self.get_attribute("class")
    }

    /// Get the href attribute (for <a> tags).
    pub fn href(&self) -> Option<&str> {
        self.get_attribute("href")
    }

    /// Get the src attribute (for <img>, <script>, etc.).
    pub fn src(&self) -> Option<&str> {
        self.get_attribute("src")
    }
}
