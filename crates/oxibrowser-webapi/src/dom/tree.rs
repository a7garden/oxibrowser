//! DOM tree structure with parent-child relationships.

use crate::dom::node::NodeId;
use std::collections::HashMap;

/// A DOM tree with parent-child relationships stored as adjacency lists.
#[derive(Debug, Clone)]
pub struct Tree {
    /// Parent of each node (None for root).
    parents: HashMap<NodeId, NodeId>,
    /// Children of each node.
    children: HashMap<NodeId, Vec<NodeId>>,
    /// Root node ID.
    root: Option<NodeId>,
}

impl Tree {
    /// Create an empty tree.
    pub fn new() -> Self {
        Self {
            parents: HashMap::new(),
            children: HashMap::new(),
            root: None,
        }
    }

    /// Set the root node.
    pub fn set_root(&mut self, id: NodeId) {
        self.root = Some(id);
    }

    /// Get the root node ID.
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }

    /// Add a child node to a parent.
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.parents.insert(child, parent);
        self.children.entry(parent).or_default().push(child);
    }

    /// Get the parent of a node.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.parents.get(&id).copied()
    }

    /// Get the children of a node.
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.children.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get the first child of a node.
    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.children.get(&id).and_then(|v| v.first()).copied()
    }

    /// Get the last child of a node.
    pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.children.get(&id).and_then(|v| v.last()).copied()
    }

    /// Traverse the tree depth-first.
    pub fn traverse_dfs<F>(&self, start: NodeId, visitor: &mut F)
    where
        F: FnMut(NodeId),
    {
        visitor(start);
        if let Some(children) = self.children.get(&start) {
            for &child in children {
                self.traverse_dfs(child, visitor);
            }
        }
    }

    /// Traverse the tree breadth-first.
    pub fn traverse_bfs<F>(&self, start: NodeId, visitor: &mut F)
    where
        F: FnMut(NodeId),
    {
        let mut queue = vec![start];
        while let Some(current) = queue.pop() {
            visitor(current);
            if let Some(children) = self.children.get(&current) {
                for &child in children.iter().rev() {
                    queue.push(child);
                }
            }
        }
    }
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}
