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
    Doctype { name: String },
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

    /// Set an attribute on this node (element nodes only).
    ///
    /// If the attribute already exists, its value is updated.
    /// If this is not an element node, this is a no-op.
    pub fn set_attribute(&mut self, name: &str, value: &str) {
        if let NodeType::Element { attributes, .. } = &mut self.node_type {
            if let Some(entry) = attributes
                .iter_mut()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
            {
                entry.1 = value.to_string();
            } else {
                attributes.push((name.to_string(), value.to_string()));
            }
        }
    }

    /// Set the `value` property on this node.
    ///
    /// Stores the value as a special `"value"` attribute on element nodes.
    pub fn set_value(&mut self, value: &str) {
        self.set_attribute("value", value);
    }

    /// Set the text content of this node.
    ///
    /// For text nodes, replaces the text directly.
    /// For element nodes, updates or creates a text child representation
    /// via a special `"data-oxi-text"` attribute (the actual DOM text
    /// mutation would require tree surgery, which we defer).
    pub fn set_text_content(&mut self, text: &str) {
        match &mut self.node_type {
            NodeType::Text(ref mut t) => {
                *t = text.to_string();
            }
            NodeType::Element { attributes, .. } => {
                // Store as a data attribute so it round-trips through snapshot.
                if let Some(entry) = attributes.iter_mut().find(|(k, _)| k == "data-oxi-text") {
                    entry.1 = text.to_string();
                } else {
                    attributes.push(("data-oxi-text".to_string(), text.to_string()));
                }
            }
            _ => {}
        }
    }
}
