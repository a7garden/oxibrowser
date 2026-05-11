//! DOM implementation backed by html5ever.
//!
//! Provides Document, Node, and basic DOM tree operations using
//! html5ever (Servo's HTML parser).

mod document;
mod node;
mod tree;

pub use document::Document;
pub use document::{ResourceUrl, ResourceKind};
pub use node::{Node, NodeId, NodeType};
pub use tree::Tree;
