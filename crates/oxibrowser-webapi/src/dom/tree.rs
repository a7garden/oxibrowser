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
    ///
    /// If the child already has a different parent, it is removed from the
    /// old parent's children list first (reparenting).
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        // Remove from old parent if reparenting
        if let Some(old_parent) = self.parents.get(&child).copied()
            && old_parent != parent
            && let Some(children) = self.children.get_mut(&old_parent)
        {
            children.retain(|&c| c != child);
        }
        self.parents.insert(child, parent);
        self.children.entry(parent).or_default().push(child);
    }

    /// Get the parent of a node.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.parents.get(&id).copied()
    }

    /// Remove the parent association for a node (detach from parent).
    pub fn remove_parent(&mut self, id: NodeId) {
        self.parents.remove(&id);
    }

    /// Get a mutable reference to the children of a node.
    pub fn children_mut(&mut self, id: NodeId) -> Option<&mut Vec<NodeId>> {
        self.children.get_mut(&id)
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

    /// Traverse the tree breadth-first using a FIFO queue (VecDeque).
    pub fn traverse_bfs<F>(&self, start: NodeId, visitor: &mut F)
    where
        F: FnMut(NodeId),
    {
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(start);
        while let Some(current) = queue.pop_front() {
            visitor(current);
            if let Some(children) = self.children.get(&current) {
                for &child in children {
                    queue.push_back(child);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_basic() {
        let mut tree = Tree::new();
        let root = NodeId(0);
        let child1 = NodeId(1);
        let child2 = NodeId(2);

        tree.set_root(root);
        tree.append_child(root, child1);
        tree.append_child(root, child2);

        assert_eq!(tree.root(), Some(root));
        assert_eq!(tree.parent(child1), Some(root));
        assert_eq!(tree.parent(child2), Some(root));
        assert_eq!(tree.children(root), &[child1, child2]);
        assert_eq!(tree.first_child(root), Some(child1));
        assert_eq!(tree.last_child(root), Some(child2));
    }

    #[test]
    fn test_tree_traversal_dfs() {
        let mut tree = Tree::new();
        // Build:
        //     0
        //    / \
        //   1   2
        //  / \
        // 3   4
        let root = NodeId(0);
        tree.set_root(root);
        tree.append_child(root, NodeId(1));
        tree.append_child(root, NodeId(2));
        tree.append_child(NodeId(1), NodeId(3));
        tree.append_child(NodeId(1), NodeId(4));

        let mut order = Vec::new();
        tree.traverse_dfs(root, &mut |id| order.push(id.0));

        assert_eq!(order, vec![0, 1, 3, 4, 2], "DFS should visit in pre-order");
    }

    #[test]
    fn test_tree_traversal_bfs() {
        let mut tree = Tree::new();
        // Same tree:
        //     0
        //    / \
        //   1   2
        //  / \
        // 3   4
        let root = NodeId(0);
        tree.set_root(root);
        tree.append_child(root, NodeId(1));
        tree.append_child(root, NodeId(2));
        tree.append_child(NodeId(1), NodeId(3));
        tree.append_child(NodeId(1), NodeId(4));

        let mut order = Vec::new();
        tree.traverse_bfs(root, &mut |id| order.push(id.0));

        // BFS should visit level by level: 0, 1, 2, 3, 4
        assert_eq!(
            order,
            vec![0, 1, 2, 3, 4],
            "BFS should visit in breadth-first order (level by level)"
        );
    }

    #[test]
    fn test_tree_empty() {
        let tree = Tree::new();
        assert_eq!(tree.root(), None);
        assert!(tree.children(NodeId(0)).is_empty());
        assert_eq!(tree.parent(NodeId(0)), None);
        assert_eq!(tree.first_child(NodeId(0)), None);
        assert_eq!(tree.last_child(NodeId(0)), None);
    }
}
