//! Channel management

use crate::tree::ChannelTree;
use tslib_core::state::{Channel, MaxClientsKind};
use std::sync::Arc;
use tokio::sync::RwLock;

/// High-level channel manager
pub struct ChannelManager {
    /// Channel tree
    tree: Arc<RwLock<ChannelTree>>,
    /// Current channel ID
    current_channel: Arc<RwLock<Option<u64>>>,
}

impl ChannelManager {
    /// Create a new channel manager
    pub fn new() -> Self {
        Self {
            tree: Arc::new(RwLock::new(ChannelTree::new())),
            current_channel: Arc::new(RwLock::new(None)),
        }
    }

    /// Get the channel tree
    pub async fn tree(&self) -> ChannelTree {
        self.tree.read().await.clone()
    }

    /// Set the channel tree
    pub async fn set_tree(&self, tree: ChannelTree) {
        *self.tree.write().await = tree;
    }

    /// Add a channel
    pub async fn add_channel(&self, channel: Channel) {
        self.tree.write().await.add_channel(channel);
    }

    /// Remove a channel
    pub async fn remove_channel(&self, id: u64) -> Option<Channel> {
        self.tree.write().await.remove_channel(id)
    }

    /// Update a channel
    pub async fn update_channel(&self, channel: Channel) {
        self.tree.write().await.update_channel(channel);
    }

    /// Get a channel by ID
    pub async fn get_channel(&self, id: u64) -> Option<Channel> {
        self.tree.read().await.get(id).cloned()
    }

    /// Get current channel
    pub async fn current_channel(&self) -> Option<u64> {
        *self.current_channel.read().await
    }

    /// Set current channel
    pub async fn set_current_channel(&self, id: u64) {
        *self.current_channel.write().await = Some(id);
    }

    /// Find channel by name
    pub async fn find_by_name(&self, name: &str) -> Option<Channel> {
        self.tree.read().await.find_by_name(name).cloned()
    }

    /// Get root channels
    pub async fn root_channels(&self) -> Vec<Channel> {
        self.tree.read().await.roots().into_iter().cloned().collect()
    }

    /// Get children of a channel
    pub async fn children(&self, parent_id: u64) -> Vec<Channel> {
        self.tree.read().await.children(parent_id).into_iter().cloned().collect()
    }

    /// Get path to a channel
    pub async fn path_to(&self, id: u64) -> Vec<Channel> {
        self.tree.read().await.path_to(id).into_iter().cloned().collect()
    }

    /// Get all channels
    pub async fn all_channels(&self) -> Vec<Channel> {
        self.tree.read().await.all_channels().into_iter().cloned().collect()
    }

    /// Get joinable channels (not full, no password or we know it)
    ///
    /// This cannot check capacity because the manager does not track users;
    /// use [`Self::joinable_channels_for_users`] or
    /// [`Self::joinable_channels_with_occupancy`] when member counts are
    /// available.
    #[deprecated(
        note = "use joinable_channels_for_users or joinable_channels_with_occupancy"
    )]
    pub async fn joinable_channels(&self) -> Vec<Channel> {
        self.tree
            .read()
            .await
            .find_all(|c| !c.has_password && !c.is_spacer())
            .into_iter()
            .cloned()
            .collect()
    }

    /// Get joinable channels, excluding channels that are full for the given
    /// users.
    pub async fn joinable_channels_for_users(&self, users: &[tslib_core::User]) -> Vec<Channel> {
        use std::collections::HashMap;

        let mut occupied: HashMap<u64, usize> = HashMap::new();
        for user in users {
            *occupied.entry(user.channel_id).or_insert(0) += 1;
        }
        self.joinable_channels_with_occupancy(|id| occupied.get(&id).copied().unwrap_or(0))
            .await
    }

    /// Get joinable channels, excluding channels whose direct member count
    /// (provided by `occupied`) reaches their effective `max_clients`.
    ///
    /// `Inherited` limits are resolved through the channel tree.
    pub async fn joinable_channels_with_occupancy<F>(&self, occupied: F) -> Vec<Channel>
    where
        F: Fn(u64) -> usize,
    {
        let tree = self.tree.read().await;
        tree.find_all(|c| {
            !c.has_password && !c.is_spacer() && !tree.is_full(c.id, occupied(c.id))
        })
        .into_iter()
        .cloned()
        .collect()
    }

    /// Print tree structure
    pub async fn print_tree(&self) -> String {
        self.tree.read().await.print_tree()
    }
}

impl Default for ChannelManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Channel creation builder
#[derive(Debug, Clone, Default)]
pub struct ChannelBuilder {
    name: Option<String>,
    parent_id: u64,
    topic: Option<String>,
    description: Option<String>,
    password: Option<String>,
    codec: u8,
    codec_quality: u8,
    max_clients: i32,
    is_permanent: bool,
    is_semi_permanent: bool,
    order: u64,
}

impl ChannelBuilder {
    /// Create a new channel builder
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            codec: 4,          // Opus Voice
            codec_quality: 6,  // Medium quality
            max_clients: -1,   // Unlimited
            ..Default::default()
        }
    }

    /// Set parent channel
    pub fn parent(mut self, parent_id: u64) -> Self {
        self.parent_id = parent_id;
        self
    }

    /// Set topic
    pub fn topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// Set description
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set password
    pub fn password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Set codec (4 = Opus Voice, 5 = Opus Music)
    pub fn codec(mut self, codec: u8) -> Self {
        self.codec = codec;
        self
    }

    /// Set codec quality (1-10)
    pub fn codec_quality(mut self, quality: u8) -> Self {
        self.codec_quality = quality.clamp(1, 10);
        self
    }

    /// Set max clients (-1 for unlimited)
    pub fn max_clients(mut self, max: i32) -> Self {
        self.max_clients = max;
        self
    }

    /// Make channel permanent
    pub fn permanent(mut self) -> Self {
        self.is_permanent = true;
        self.is_semi_permanent = false;
        self
    }

    /// Make channel semi-permanent
    pub fn semi_permanent(mut self) -> Self {
        self.is_semi_permanent = true;
        self.is_permanent = false;
        self
    }

    /// Make channel temporary (default)
    pub fn temporary(mut self) -> Self {
        self.is_permanent = false;
        self.is_semi_permanent = false;
        self
    }

    /// Set channel order
    pub fn order(mut self, order: u64) -> Self {
        self.order = order;
        self
    }

    /// Build the channel (for local use, not server creation)
    pub fn build(self) -> Option<Channel> {
        let max_clients_kind = if self.max_clients < 0 {
            MaxClientsKind::Unlimited
        } else {
            MaxClientsKind::Limited(self.max_clients as u16)
        };

        Some(Channel {
            id: 0, // Will be assigned by server
            parent_id: self.parent_id,
            name: self.name?,
            topic: self.topic,
            description: self.description,
            order: self.order,
            is_permanent: self.is_permanent,
            is_semi_permanent: self.is_semi_permanent,
            is_default: false,
            has_password: self.password.is_some(),
            codec: self.codec,
            codec_quality: self.codec_quality,
            max_clients: self.max_clients,
            max_clients_kind,
            max_family_clients: -1,
            max_family_clients_kind: MaxClientsKind::Unlimited,
            needed_talk_power: 0,
            icon_id: 0,
            is_subscribed: false,
            permission_hints: 0,
            permission_hints_known: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(id: u64, parent_id: u64, name: &str) -> Channel {
        Channel {
            id,
            parent_id,
            name: name.to_string(),
            max_clients: -1,
            ..Default::default()
        }
    }

    // --- ChannelBuilder tests ---

    #[test]
    fn builder_defaults() {
        let b = ChannelBuilder::new("Test");
        let c = b.build().unwrap();
        assert_eq!(c.name, "Test");
        assert_eq!(c.codec, 4); // Opus Voice
        assert_eq!(c.codec_quality, 6);
        assert_eq!(c.max_clients, -1);
        assert!(!c.is_permanent);
        assert!(!c.is_semi_permanent);
        assert!(!c.has_password);
    }

    #[test]
    fn builder_no_name_returns_none() {
        let b = ChannelBuilder::default();
        assert!(b.build().is_none());
    }

    #[test]
    fn builder_codec_quality_clamp_low() {
        let c = ChannelBuilder::new("X").codec_quality(0).build().unwrap();
        assert_eq!(c.codec_quality, 1);
    }

    #[test]
    fn builder_codec_quality_clamp_high() {
        let c = ChannelBuilder::new("X").codec_quality(20).build().unwrap();
        assert_eq!(c.codec_quality, 10);
    }

    #[test]
    fn builder_codec_quality_normal() {
        let c = ChannelBuilder::new("X").codec_quality(8).build().unwrap();
        assert_eq!(c.codec_quality, 8);
    }

    #[test]
    fn builder_permanent_flag() {
        let c = ChannelBuilder::new("X").permanent().build().unwrap();
        assert!(c.is_permanent);
        assert!(!c.is_semi_permanent);
    }

    #[test]
    fn builder_semi_permanent_flag() {
        let c = ChannelBuilder::new("X").semi_permanent().build().unwrap();
        assert!(c.is_semi_permanent);
        assert!(!c.is_permanent);
    }

    #[test]
    fn builder_temporary_resets_both() {
        let c = ChannelBuilder::new("X").permanent().temporary().build().unwrap();
        assert!(!c.is_permanent);
        assert!(!c.is_semi_permanent);
    }

    #[test]
    fn builder_permanent_overrides_semi() {
        let c = ChannelBuilder::new("X").semi_permanent().permanent().build().unwrap();
        assert!(c.is_permanent);
        assert!(!c.is_semi_permanent);
    }

    #[test]
    fn builder_password_sets_has_password() {
        let c = ChannelBuilder::new("X").password("secret").build().unwrap();
        assert!(c.has_password);
    }

    #[test]
    fn builder_parent_id() {
        let c = ChannelBuilder::new("X").parent(42).build().unwrap();
        assert_eq!(c.parent_id, 42);
    }

    #[test]
    fn builder_topic_and_description() {
        let c = ChannelBuilder::new("X")
            .topic("t")
            .description("d")
            .build()
            .unwrap();
        assert_eq!(c.topic.unwrap(), "t");
        assert_eq!(c.description.unwrap(), "d");
    }

    // --- ChannelManager tests (async) ---

    #[tokio::test]
    async fn manager_add_and_get() {
        let mgr = ChannelManager::new();
        mgr.add_channel(ch(1, 0, "Root")).await;
        let c = mgr.get_channel(1).await;
        assert_eq!(c.unwrap().name, "Root");
        assert!(mgr.get_channel(99).await.is_none());
    }

    #[tokio::test]
    async fn manager_remove() {
        let mgr = ChannelManager::new();
        mgr.add_channel(ch(1, 0, "Root")).await;
        let removed = mgr.remove_channel(1).await;
        assert!(removed.is_some());
        assert!(mgr.get_channel(1).await.is_none());
    }

    #[tokio::test]
    async fn manager_current_channel() {
        let mgr = ChannelManager::new();
        assert!(mgr.current_channel().await.is_none());
        mgr.set_current_channel(42).await;
        assert_eq!(mgr.current_channel().await, Some(42));
    }

    #[tokio::test]
    async fn manager_find_by_name() {
        let mgr = ChannelManager::new();
        mgr.add_channel(ch(1, 0, "Lobby")).await;
        assert!(mgr.find_by_name("Lobby").await.is_some());
        assert!(mgr.find_by_name("Nope").await.is_none());
    }

    #[tokio::test]
    async fn manager_joinable_excludes_spacers_and_passwords() {
        let mgr = ChannelManager::new();
        mgr.add_channel(ch(1, 0, "Normal")).await;
        mgr.add_channel(Channel {
            id: 2,
            name: "[spacer0]---".into(),
            ..Default::default()
        }).await;
        mgr.add_channel(Channel {
            id: 3,
            name: "Locked".into(),
            has_password: true,
            ..Default::default()
        }).await;
        let joinable = mgr.joinable_channels_with_occupancy(|_| 0).await;
        assert_eq!(joinable.len(), 1);
        assert_eq!(joinable[0].name, "Normal");
    }

    #[tokio::test]
    async fn manager_joinable_excludes_full_channels() {
        let mgr = ChannelManager::new();
        mgr.add_channel(Channel {
            id: 1,
            name: "Full".into(),
            max_clients: 1,
            max_clients_kind: MaxClientsKind::Limited(1),
            ..Default::default()
        })
        .await;
        mgr.add_channel(Channel {
            id: 2,
            name: "Open".into(),
            max_clients: 5,
            max_clients_kind: MaxClientsKind::Limited(5),
            ..Default::default()
        })
        .await;

        let joinable = mgr
            .joinable_channels_with_occupancy(|id| if id == 1 { 1 } else { 0 })
            .await;
        assert_eq!(joinable.len(), 1);
        assert_eq!(joinable[0].id, 2);

        let users = vec![
            tslib_core::User {
                id: 1,
                channel_id: 2,
                ..Default::default()
            };
            5
        ];
        let joinable = mgr.joinable_channels_for_users(&users).await;
        assert_eq!(joinable.len(), 1);
        assert_eq!(joinable[0].id, 1);
    }

    #[tokio::test]
    async fn manager_root_channels() {
        let mgr = ChannelManager::new();
        mgr.add_channel(ch(1, 0, "R1")).await;
        mgr.add_channel(ch(2, 0, "R2")).await;
        mgr.add_channel(ch(3, 1, "Child")).await;
        let roots = mgr.root_channels().await;
        assert_eq!(roots.len(), 2);
    }
}
