//! Channel tree structure

use tslib_core::state::{Channel, MaxClientsKind};
use std::collections::{HashMap, HashSet};

/// Order siblings by the TS3 `channel_order` predecessor chain.
///
/// `order` is the id of the preceding sibling, not a numeric sort weight.
/// `0` (or a predecessor outside this sibling set) marks a chain head. Any
/// channels left over (cycles, duplicate predecessors) are appended by id so
/// the result is deterministic and never loops.
fn order_siblings(channels: &HashMap<u64, Channel>, siblings: &mut Vec<u64>) {
    if siblings.len() < 2 {
        return;
    }

    let ids: HashSet<u64> = siblings.iter().copied().collect();
    let mut next: HashMap<u64, u64> = HashMap::new();
    let mut heads = Vec::new();

    for &id in siblings.iter() {
        let previous = channels.get(&id).map(|channel| channel.order).unwrap_or(0);
        if previous == 0 || !ids.contains(&previous) {
            heads.push(id);
        } else {
            next.entry(previous).or_insert(id);
        }
    }

    heads.sort_unstable();
    let mut ordered = Vec::with_capacity(siblings.len());
    let mut used = HashSet::new();

    for head in heads {
        let mut current = head;
        while used.insert(current) {
            ordered.push(current);
            match next.get(&current) {
                Some(&next_id) if !used.contains(&next_id) => current = next_id,
                _ => break,
            }
        }
    }

    let mut leftover: Vec<u64> = siblings
        .iter()
        .copied()
        .filter(|id| !used.contains(id))
        .collect();
    leftover.sort_unstable();
    ordered.extend(leftover);
    *siblings = ordered;
}

/// Channel tree representation
#[derive(Debug, Clone)]
pub struct ChannelTree {
    /// All channels indexed by ID
    channels: HashMap<u64, Channel>,
    /// Root channel IDs
    roots: Vec<u64>,
    /// Children by parent ID
    children: HashMap<u64, Vec<u64>>,
}

impl ChannelTree {
    /// Create an empty channel tree
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
            roots: Vec::new(),
            children: HashMap::new(),
        }
    }

    /// Build a tree from a list of channels
    pub fn from_channels(channels: Vec<Channel>) -> Self {
        let mut tree = Self::new();
        for channel in channels {
            tree.add_channel(channel);
        }
        tree.rebuild_structure();
        tree
    }

    /// Add a channel to the tree
    pub fn add_channel(&mut self, channel: Channel) {
        self.channels.insert(channel.id, channel);
        self.rebuild_structure();
    }

    /// Remove a channel and its whole subtree from the tree.
    ///
    /// Returns the removed root channel (the first entry of
    /// [`Self::remove_subtree`]). The server deletes channel subtrees, so
    /// keeping descendants would leave an inconsistent local tree.
    pub fn remove_channel(&mut self, id: u64) -> Option<Channel> {
        let mut removed = self.remove_subtree(id);
        if removed.is_empty() {
            None
        } else {
            Some(removed.remove(0))
        }
    }

    /// Remove a channel and all of its descendants.
    ///
    /// Returns the removed channels in pre-order (root first). Unknown ids
    /// return an empty list.
    pub fn remove_subtree(&mut self, id: u64) -> Vec<Channel> {
        let mut ids = Vec::new();
        let mut visited = HashSet::new();
        self.collect_subtree_ids(id, &mut ids, &mut visited);

        let mut removed = Vec::with_capacity(ids.len());
        for channel_id in ids {
            if let Some(channel) = self.channels.remove(&channel_id) {
                removed.push(channel);
            }
        }
        if !removed.is_empty() {
            self.rebuild_structure();
        }
        removed
    }

    fn collect_subtree_ids(&self, id: u64, ids: &mut Vec<u64>, visited: &mut HashSet<u64>) {
        if !self.channels.contains_key(&id) || !visited.insert(id) {
            return;
        }
        ids.push(id);
        if let Some(children) = self.children.get(&id) {
            for &child in children {
                self.collect_subtree_ids(child, ids, visited);
            }
        }
    }

    /// Update a channel
    ///
    /// Reparenting under one of its own descendants would create a cycle; in
    /// that case the old parent is kept and the rest of the update applies.
    pub fn update_channel(&mut self, channel: Channel) {
        let mut channel = channel;

        if channel.parent_id != 0 && self.would_create_cycle(channel.id, channel.parent_id) {
            let old_parent = self
                .channels
                .get(&channel.id)
                .map(|c| c.parent_id)
                .unwrap_or(0);
            tracing::warn!(
                "Channel {} cannot be reparented under its descendant {}; keeping parent {}",
                channel.id,
                channel.parent_id,
                old_parent
            );
            channel.parent_id = old_parent;
        }

        self.channels.insert(channel.id, channel);
        // Rebuild instead of patching child lists: this also wires up a
        // channel that was not in the tree yet and prevents duplicates.
        self.rebuild_structure();
    }

    fn would_create_cycle(&self, id: u64, new_parent: u64) -> bool {
        if id == new_parent {
            return true;
        }

        let mut current = new_parent;
        let mut visited = 0usize;
        while current != 0 {
            if current == id {
                return true;
            }
            visited += 1;
            if visited > self.channels.len() {
                // Existing broken data: treat as a cycle.
                return true;
            }
            match self.channels.get(&current) {
                Some(channel) => current = channel.parent_id,
                None => break,
            }
        }
        false
    }

    /// Get a channel by ID
    pub fn get(&self, id: u64) -> Option<&Channel> {
        self.channels.get(&id)
    }

    /// Resolve the effective member limit, following `Inherited` up the
    /// parent chain.
    ///
    /// Falls back to the legacy `max_clients` when the structured kind is
    /// `Unknown` (for example channel objects from older serialized data).
    /// A root without an explicit limit inherits the server default, which is
    /// unlimited.
    pub fn effective_max_clients(&self, id: u64) -> MaxClientsKind {
        let mut current = self.channels.get(&id);
        let mut visited = 0usize;

        while let Some(channel) = current {
            match channel.max_clients_kind {
                MaxClientsKind::Limited(_) | MaxClientsKind::Unlimited => {
                    return channel.max_clients_kind;
                }
                MaxClientsKind::Unknown => {
                    return if channel.max_clients < 0 {
                        MaxClientsKind::Unknown
                    } else {
                        MaxClientsKind::Limited(channel.max_clients as u16)
                    };
                }
                MaxClientsKind::Inherited => {}
            }

            if channel.parent_id == 0 || channel.parent_id == channel.id {
                break;
            }
            visited += 1;
            if visited > self.channels.len() {
                break;
            }
            current = self.channels.get(&channel.parent_id);
        }

        MaxClientsKind::Unlimited
    }

    /// Check whether a channel is full given its current direct member count.
    ///
    /// `Unknown` limits are treated as not full, matching the legacy
    /// `max_clients == -1` behaviour.
    pub fn is_full(&self, id: u64, occupied: usize) -> bool {
        match self.effective_max_clients(id) {
            MaxClientsKind::Limited(limit) => occupied >= limit as usize,
            _ => false,
        }
    }

    /// Get root channels
    pub fn roots(&self) -> Vec<&Channel> {
        self.roots
            .iter()
            .filter_map(|id| self.channels.get(id))
            .collect()
    }

    /// Get children of a channel
    pub fn children(&self, parent_id: u64) -> Vec<&Channel> {
        self.children
            .get(&parent_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.channels.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get parent of a channel
    pub fn parent(&self, id: u64) -> Option<&Channel> {
        self.channels
            .get(&id)
            .and_then(|c| self.channels.get(&c.parent_id))
    }

    /// Get the path from root to a channel
    pub fn path_to(&self, id: u64) -> Vec<&Channel> {
        let mut path = Vec::new();
        let mut current_id = id;
        let mut visited = HashSet::new();

        while let Some(channel) = self.channels.get(&current_id) {
            if !visited.insert(channel.id) {
                break;
            }
            path.push(channel);
            if channel.parent_id == 0 {
                break;
            }
            current_id = channel.parent_id;
        }

        path.reverse();
        path
    }

    /// Find a channel by name
    pub fn find_by_name(&self, name: &str) -> Option<&Channel> {
        self.channels.values().find(|c| c.name == name)
    }

    /// Find channels matching a predicate
    pub fn find_all<F>(&self, predicate: F) -> Vec<&Channel>
    where
        F: Fn(&Channel) -> bool,
    {
        self.channels.values().filter(|c| predicate(c)).collect()
    }

    /// Get all channels as a flat list
    pub fn all_channels(&self) -> Vec<&Channel> {
        self.channels.values().collect()
    }

    /// Get channel count
    pub fn len(&self) -> usize {
        self.channels.len()
    }

    /// Check if tree is empty
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// Rebuild internal structure
    fn rebuild_structure(&mut self) {
        self.roots.clear();
        self.children.clear();

        for channel in self.channels.values() {
            if channel.parent_id == 0 {
                self.roots.push(channel.id);
            } else {
                self.children
                    .entry(channel.parent_id)
                    .or_default()
                    .push(channel.id);
            }
        }

        // Sort roots by the predecessor chain
        order_siblings(&self.channels, &mut self.roots);

        // Sort children by the predecessor chain
        for children in self.children.values_mut() {
            order_siblings(&self.channels, children);
        }
    }

    /// Print the tree structure (for debugging)
    pub fn print_tree(&self) -> String {
        let mut output = String::new();
        let mut visited = HashSet::new();
        for root in &self.roots {
            self.print_subtree(*root, 0, &mut visited, &mut output);
        }
        output
    }

    fn print_subtree(
        &self,
        id: u64,
        depth: usize,
        visited: &mut HashSet<u64>,
        output: &mut String,
    ) {
        if !visited.insert(id) {
            return;
        }
        if let Some(channel) = self.channels.get(&id) {
            let indent = "  ".repeat(depth);
            output.push_str(&format!("{}{}\n", indent, channel.name));

            if let Some(children) = self.children.get(&id) {
                for &child_id in children {
                    self.print_subtree(child_id, depth + 1, visited, output);
                }
            }
        }
    }
}

impl Default for ChannelTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(id: u64, parent_id: u64, name: &str, order: u64) -> Channel {
        Channel {
            id,
            parent_id,
            name: name.to_string(),
            order,
            ..Default::default()
        }
    }

    #[test]
    fn new_tree_is_empty() {
        let tree = ChannelTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
    }

    #[test]
    fn add_root_channel() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "Root", 0));
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.roots().len(), 1);
        assert_eq!(tree.roots()[0].name, "Root");
    }

    #[test]
    fn add_child_channel() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "Root", 0));
        tree.add_channel(ch(2, 1, "Child", 0));
        assert_eq!(tree.children(1).len(), 1);
        assert_eq!(tree.children(1)[0].name, "Child");
    }

    #[test]
    fn remove_channel() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "Root", 0));
        let removed = tree.remove_channel(1);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().name, "Root");
        assert!(tree.is_empty());
    }

    #[test]
    fn remove_nonexistent_returns_none() {
        let mut tree = ChannelTree::new();
        assert!(tree.remove_channel(99).is_none());
    }

    #[test]
    fn remove_subtree_removes_descendants() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "Root", 0));
        tree.add_channel(ch(2, 1, "Child", 0));
        tree.add_channel(ch(3, 2, "Grandchild", 0));
        tree.add_channel(ch(4, 0, "Sibling", 1));

        let removed = tree.remove_subtree(1);

        assert_eq!(removed.len(), 3);
        assert_eq!(removed[0].id, 1);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.roots()[0].id, 4);
    }

    #[test]
    fn update_inserts_channel_not_in_tree() {
        let mut tree = ChannelTree::new();
        tree.update_channel(ch(1, 0, "New", 0));
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.roots().len(), 1);
    }

    #[test]
    fn update_rejects_cycle() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "Root", 0));
        tree.add_channel(ch(2, 1, "Child", 0));

        // Reparenting the root under its own child must be ignored.
        tree.update_channel(ch(1, 2, "Root", 0));

        assert_eq!(tree.get(1).unwrap().parent_id, 0);
        assert_eq!(tree.roots().len(), 1);
        assert_eq!(tree.children(2).len(), 0);
    }

    #[test]
    fn path_and_print_terminate_on_cycle() {
        let tree = ChannelTree::from_channels(vec![ch(1, 2, "A", 0), ch(2, 1, "B", 0)]);

        let path = tree.path_to(1);
        assert!(path.len() <= 2);

        // Neither channel is a root, so the printable tree is empty; the
        // important part is that it terminates.
        assert!(tree.print_tree().lines().count() <= 2);
    }

    #[test]
    fn update_channel_same_parent() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "Old", 0));
        tree.update_channel(ch(1, 0, "New", 0));
        assert_eq!(tree.get(1).unwrap().name, "New");
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn update_channel_reparent() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "Root1", 0));
        tree.add_channel(ch(2, 0, "Root2", 1));
        tree.add_channel(ch(3, 1, "Child", 0));
        // Move child from Root1 to Root2
        tree.update_channel(ch(3, 2, "Child", 0));
        assert_eq!(tree.children(1).len(), 0);
        assert_eq!(tree.children(2).len(), 1);
    }

    #[test]
    fn get_channel() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(5, 0, "Test", 0));
        assert_eq!(tree.get(5).unwrap().name, "Test");
        assert!(tree.get(99).is_none());
    }

    #[test]
    fn parent_lookup() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "Root", 0));
        tree.add_channel(ch(2, 1, "Child", 0));
        assert_eq!(tree.parent(2).unwrap().name, "Root");
        assert!(tree.parent(1).is_none()); // root has no parent (parent_id=0)
    }

    #[test]
    fn path_to_channel() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "Root", 0));
        tree.add_channel(ch(2, 1, "Mid", 0));
        tree.add_channel(ch(3, 2, "Leaf", 0));
        let path = tree.path_to(3);
        let names: Vec<&str> = path.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Root", "Mid", "Leaf"]);
    }

    #[test]
    fn path_to_root() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "Root", 0));
        let path = tree.path_to(1);
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].name, "Root");
    }

    #[test]
    fn find_by_name() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "Lobby", 0));
        tree.add_channel(ch(2, 0, "Music", 1));
        assert_eq!(tree.find_by_name("Music").unwrap().id, 2);
        assert!(tree.find_by_name("Nope").is_none());
    }

    #[test]
    fn find_all_with_predicate() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "A", 0));
        tree.add_channel(ch(2, 0, "B", 1));
        tree.add_channel(ch(3, 0, "AB", 2));
        let results = tree.find_all(|c| c.name.contains('A'));
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn all_channels_flat() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(1, 0, "R", 0));
        tree.add_channel(ch(2, 1, "C", 0));
        assert_eq!(tree.all_channels().len(), 2);
    }

    #[test]
    fn from_channels_builds_structure() {
        let channels = vec![
            ch(1, 0, "Root", 0),
            ch(2, 1, "Child1", 0),
            ch(3, 1, "Child2", 1),
        ];
        let tree = ChannelTree::from_channels(channels);
        assert_eq!(tree.roots().len(), 1);
        assert_eq!(tree.children(1).len(), 2);
    }

    #[test]
    fn print_tree_output() {
        let channels = vec![
            ch(1, 0, "Root", 0),
            ch(2, 1, "Child", 0),
        ];
        let tree = ChannelTree::from_channels(channels);
        let output = tree.print_tree();
        assert!(output.contains("Root"));
        assert!(output.contains("  Child"));
    }

    #[test]
    fn from_channels_sorts_by_order() {
        let channels = vec![
            ch(1, 0, "B", 1),
            ch(2, 0, "A", 0),
        ];
        let tree = ChannelTree::from_channels(channels);
        let roots = tree.roots();
        assert_eq!(roots[0].name, "A");
        assert_eq!(roots[1].name, "B");
    }

    #[test]
    fn roots_follow_predecessor_chain() {
        let mut tree = ChannelTree::new();
        // Insert out of order; chain is 1 -> 2 -> 3.
        tree.add_channel(ch(3, 0, "C", 2));
        tree.add_channel(ch(1, 0, "A", 0));
        tree.add_channel(ch(2, 0, "B", 1));

        let names: Vec<_> = tree.roots().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C"]);
    }

    #[test]
    fn children_follow_predecessor_chain() {
        let mut tree = ChannelTree::new();
        tree.add_channel(ch(10, 0, "Root", 0));
        tree.add_channel(ch(13, 10, "C", 12));
        tree.add_channel(ch(11, 10, "A", 0));
        tree.add_channel(ch(12, 10, "B", 11));

        let names: Vec<_> = tree.children(10).iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C"]);
    }

    #[test]
    fn order_cycle_does_not_loop() {
        let tree = ChannelTree::from_channels(vec![
            ch(1, 0, "A", 2),
            ch(2, 0, "B", 1),
        ]);

        let names: Vec<_> = tree.roots().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["A", "B"]);
    }

    #[test]
    fn missing_predecessor_is_a_head() {
        let tree = ChannelTree::from_channels(vec![ch(1, 0, "A", 99)]);
        assert_eq!(tree.roots().len(), 1);
        assert_eq!(tree.roots()[0].name, "A");
    }

    #[test]
    fn inherited_limit_resolves_through_parent() {
        let mut tree = ChannelTree::new();
        tree.add_channel(Channel {
            id: 1,
            max_clients: 2,
            max_clients_kind: MaxClientsKind::Limited(2),
            ..Default::default()
        });
        tree.add_channel(Channel {
            id: 2,
            parent_id: 1,
            max_clients: -1,
            max_clients_kind: MaxClientsKind::Inherited,
            ..Default::default()
        });

        assert_eq!(
            tree.effective_max_clients(2),
            MaxClientsKind::Limited(2)
        );
        assert!(!tree.is_full(2, 1));
        assert!(tree.is_full(2, 2));
    }

    #[test]
    fn inherited_without_parent_limit_is_unlimited() {
        let mut tree = ChannelTree::new();
        tree.add_channel(Channel {
            id: 1,
            max_clients: -1,
            max_clients_kind: MaxClientsKind::Unlimited,
            ..Default::default()
        });
        tree.add_channel(Channel {
            id: 2,
            parent_id: 1,
            max_clients: -1,
            max_clients_kind: MaxClientsKind::Inherited,
            ..Default::default()
        });

        assert_eq!(
            tree.effective_max_clients(2),
            MaxClientsKind::Unlimited
        );
        assert!(!tree.is_full(2, 100));
    }

    #[test]
    fn unknown_limit_falls_back_to_legacy_value() {
        let mut tree = ChannelTree::new();
        tree.add_channel(Channel {
            id: 1,
            max_clients: 3,
            max_clients_kind: MaxClientsKind::Unknown,
            ..Default::default()
        });

        assert_eq!(
            tree.effective_max_clients(1),
            MaxClientsKind::Limited(3)
        );
        assert!(tree.is_full(1, 3));
        assert!(!tree.is_full(1, 2));
    }
}
