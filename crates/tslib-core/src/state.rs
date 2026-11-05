//! Server state management
//!
//! Tracks channels, users, and other server information.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Complete server state
#[derive(Debug, Clone, Default)]
pub struct ServerState {
    /// Server information
    pub server: ServerInfo,
    /// All channels on the server
    pub channels: HashMap<u64, Channel>,
    /// All users currently connected
    pub users: HashMap<u16, User>,
    /// Server groups
    pub server_groups: HashMap<u64, ServerGroup>,
    /// Channel groups
    pub channel_groups: HashMap<u64, ChannelGroup>,
}

impl ServerState {
    /// Get a channel by ID
    pub fn channel(&self, id: u64) -> Option<&Channel> {
        self.channels.get(&id)
    }

    /// Get a user by ID
    pub fn user(&self, id: u16) -> Option<&User> {
        self.users.get(&id)
    }

    /// Get all users in a specific channel
    pub fn users_in_channel(&self, channel_id: u64) -> Vec<&User> {
        self.users
            .values()
            .filter(|u| u.channel_id == channel_id)
            .collect()
    }

    /// Get the root channels (no parent)
    pub fn root_channels(&self) -> Vec<&Channel> {
        self.channels
            .values()
            .filter(|c| c.parent_id == 0)
            .collect()
    }

    /// Get child channels of a parent
    pub fn child_channels(&self, parent_id: u64) -> Vec<&Channel> {
        self.channels
            .values()
            .filter(|c| c.parent_id == parent_id)
            .collect()
    }

    /// Find a channel by name
    pub fn find_channel_by_name(&self, name: &str) -> Option<&Channel> {
        self.channels.values().find(|c| c.name == name)
    }

    /// Find a user by nickname
    pub fn find_user_by_name(&self, name: &str) -> Option<&User> {
        self.users.values().find(|u| u.nickname == name)
    }
}

/// Server information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Server unique identifier
    pub uid: String,
    /// Server name
    pub name: String,
    /// Welcome message
    pub welcome_message: Option<String>,
    /// Server platform
    pub platform: String,
    /// Server version
    pub version: String,
    /// Maximum clients allowed
    pub max_clients: u32,
    /// Current number of clients
    pub clients_online: u32,
    /// Number of channels
    pub channels_online: u32,
    /// Server uptime in seconds
    pub uptime: u64,
    /// Server icon ID
    pub icon_id: i64,
    /// Host banner URL
    pub host_banner_url: Option<String>,
    /// Host banner image URL
    pub host_banner_gfx_url: Option<String>,
}

/// Structured interpretation of a channel's `max_clients` value.
///
/// The legacy `Channel::max_clients: i32` conflates "unlimited" (-1),
/// "inherit from parent" (-1) and "not synchronized yet" (-1). This enum keeps
/// the distinction so capacity checks can resolve `Inherited` through the
/// channel tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaxClientsKind {
    /// No limit.
    Unlimited,
    /// Use the parent channel's limit.
    Inherited,
    /// Explicit limit.
    Limited(u16),
    /// The server did not send a value (or old serialized data).
    #[default]
    Unknown,
}

/// Computed channel permission hints plus whether the query succeeded.
///
/// `bits == 0, known == false` means the server did not provide permission
/// data (or the query failed); callers should show "unknown" rather than
/// assuming the user is denied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionHints {
    /// ChannelPermissionHint bitflags.
    pub bits: u64,
    /// True when the values came from a successful query.
    pub known: bool,
}

impl PermissionHints {
    /// Unknown hints (query failed or was never run).
    pub const UNKNOWN: Self = Self {
        bits: 0,
        known: false,
    };

    /// Hints from a successful query.
    pub fn known(bits: u64) -> Self {
        Self { bits, known: true }
    }
}

/// Channel information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Channel {
    /// Channel ID
    pub id: u64,
    /// Parent channel ID (0 for root)
    pub parent_id: u64,
    /// Channel name
    pub name: String,
    /// Channel topic
    pub topic: Option<String>,
    /// Channel description
    pub description: Option<String>,
    /// Channel order: the preceding channel id, not a sort weight.
    pub order: u64,
    /// Is permanent channel
    pub is_permanent: bool,
    /// Is semi-permanent channel
    pub is_semi_permanent: bool,
    /// Is default channel
    pub is_default: bool,
    /// Has password
    pub has_password: bool,
    /// Audio codec
    pub codec: u8,
    /// Codec quality (0-10)
    pub codec_quality: u8,
    /// Maximum clients (-1 for unlimited)
    pub max_clients: i32,
    /// Structured max clients value; `max_clients` is kept for compatibility
    pub max_clients_kind: MaxClientsKind,
    /// Max family clients
    pub max_family_clients: i32,
    /// Structured max family clients value
    pub max_family_clients_kind: MaxClientsKind,
    /// Needed talk power
    pub needed_talk_power: i32,
    /// Channel icon ID
    pub icon_id: i64,
    /// Is channel subscribed
    pub is_subscribed: bool,
    /// Permission hints bitflags (from ChannelPermissionHint)
    /// FILE_UPLOAD=64, FILE_DOWNLOAD=128, FILE_DELETE=256,
    /// FILE_RENAME=512, FILE_BROWSE=1024, FILE_DIRECTORY_CREATE=2048
    pub permission_hints: u64,
    /// True when `permission_hints` came from the server or a successful query
    pub permission_hints_known: bool,
}

impl Channel {
    /// Check if this is a spacer channel
    pub fn is_spacer(&self) -> bool {
        self.name.starts_with("[spacer")
            || self.name.starts_with("[cspacer")
            || self.name.starts_with("[lspacer")
            || self.name.starts_with("[rspacer")
    }

    /// Check if channel has unlimited slots
    pub fn is_unlimited(&self) -> bool {
        self.max_clients == -1
    }

    /// Check if channel is full
    ///
    /// `Inherited` limits are not resolved here because that needs the parent
    /// chain; use `ChannelTree::is_full` for an effective-limit check.
    pub fn is_full(&self, current_users: usize) -> bool {
        match self.max_clients_kind {
            MaxClientsKind::Limited(limit) => current_users >= limit as usize,
            MaxClientsKind::Unlimited => false,
            _ => {
                if self.is_unlimited() {
                    false
                } else {
                    current_users >= self.max_clients as usize
                }
            }
        }
    }
}

/// User information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct User {
    /// Client ID (session-specific)
    pub id: u16,
    /// Unique identifier (persistent)
    pub uid: String,
    /// Database ID
    pub database_id: u64,
    /// Current channel ID
    pub channel_id: u64,
    /// Nickname
    pub nickname: String,
    /// Client type (0 = normal, 1 = query)
    pub client_type: u8,
    /// Is currently talking
    pub is_talking: bool,
    /// Is input muted (microphone)
    pub is_input_muted: bool,
    /// Is output muted (speakers)
    pub is_output_muted: bool,
    /// Is input hardware available
    pub has_input_hardware: bool,
    /// Is output hardware available
    pub has_output_hardware: bool,
    /// Is away
    pub is_away: bool,
    /// Away message
    pub away_message: Option<String>,
    /// Is recording
    pub is_recording: bool,
    /// Is priority speaker
    pub is_priority_speaker: bool,
    /// Is channel commander
    pub is_channel_commander: bool,
    /// Talk power
    pub talk_power: i32,
    /// Is talker
    pub is_talker: bool,
    /// Server groups
    pub server_groups: Vec<u64>,
    /// Channel group
    pub channel_group: u64,
    /// Client platform
    pub platform: String,
    /// Client version
    pub version: String,
    /// Country code
    pub country: Option<String>,
    /// Client description
    pub description: Option<String>,
    /// Avatar ID
    pub avatar_id: Option<String>,
    /// Icon ID
    pub icon_id: i64,
    /// Idle time in milliseconds
    pub idle_time: u64,
    /// Connection time in milliseconds
    pub connected_time: u64,
    /// Account creation time (unix seconds), when the server provides it
    pub created_at: Option<i64>,
    /// Last connection time (unix seconds), when the server provides it
    pub last_connected_at: Option<i64>,
}

impl User {
    /// Check if user can talk in their current channel
    pub fn can_talk(&self) -> bool {
        !self.is_input_muted && self.has_input_hardware && self.is_talker
    }

    /// Check if user is a server query client
    pub fn is_query(&self) -> bool {
        self.client_type == 1
    }
}

/// Server group information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerGroup {
    /// Group ID
    pub id: u64,
    /// Group name
    pub name: String,
    /// Group type (0 = template, 1 = regular, 2 = query)
    pub group_type: u8,
    /// Icon ID
    pub icon_id: i64,
    /// Is the group saved to the database
    pub is_permanent: bool,
    /// Sort order
    pub sort_id: i32,
    /// Required member add power
    pub needed_member_add_power: i32,
    /// Required member remove power
    pub needed_member_remove_power: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_channel(id: u64, parent_id: u64, name: &str) -> Channel {
        Channel {
            id,
            parent_id,
            name: name.to_string(),
            max_clients: -1,
            ..Default::default()
        }
    }

    fn make_user(id: u16, channel_id: u64, nickname: &str) -> User {
        User {
            id,
            channel_id,
            nickname: nickname.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn spacer_detection() {
        assert!(Channel { name: "[spacer0]".into(), ..Default::default() }.is_spacer());
        assert!(Channel { name: "[cspacer]Center".into(), ..Default::default() }.is_spacer());
        assert!(Channel { name: "[lspacer]Left".into(), ..Default::default() }.is_spacer());
        assert!(Channel { name: "[rspacer]Right".into(), ..Default::default() }.is_spacer());
        assert!(!Channel { name: "General".into(), ..Default::default() }.is_spacer());
    }

    #[test]
    fn unlimited_channel() {
        let ch = Channel { max_clients: -1, ..Default::default() };
        assert!(ch.is_unlimited());
    }

    #[test]
    fn limited_channel() {
        let ch = Channel { max_clients: 10, ..Default::default() };
        assert!(!ch.is_unlimited());
    }

    #[test]
    fn full_channel() {
        let ch = Channel { max_clients: 2, ..Default::default() };
        assert!(ch.is_full(2));
        assert!(ch.is_full(3));
        assert!(!ch.is_full(1));
    }

    #[test]
    fn full_channel_uses_structured_limit() {
        let ch = Channel {
            max_clients: 2,
            max_clients_kind: MaxClientsKind::Limited(2),
            ..Default::default()
        };
        assert!(!ch.is_full(1));
        assert!(ch.is_full(2));

        let inherited = Channel {
            max_clients: -1,
            max_clients_kind: MaxClientsKind::Inherited,
            ..Default::default()
        };
        assert!(!inherited.is_full(100));
    }

    #[test]
    fn unlimited_never_full() {
        let ch = Channel { max_clients: -1, ..Default::default() };
        assert!(!ch.is_full(1000));
    }

    #[test]
    fn user_can_talk() {
        let u = User {
            is_input_muted: false,
            has_input_hardware: true,
            is_talker: true,
            ..Default::default()
        };
        assert!(u.can_talk());
    }

    #[test]
    fn muted_user_cannot_talk() {
        let u = User {
            is_input_muted: true,
            has_input_hardware: true,
            is_talker: true,
            ..Default::default()
        };
        assert!(!u.can_talk());
    }

    #[test]
    fn no_hardware_cannot_talk() {
        let u = User {
            is_input_muted: false,
            has_input_hardware: false,
            is_talker: true,
            ..Default::default()
        };
        assert!(!u.can_talk());
    }

    #[test]
    fn query_client_detection() {
        let normal = User { client_type: 0, ..Default::default() };
        let query = User { client_type: 1, ..Default::default() };
        assert!(!normal.is_query());
        assert!(query.is_query());
    }

    #[test]
    fn server_state_lookups() {
        let mut state = ServerState::default();
        state.channels.insert(1, make_channel(1, 0, "Root"));
        state.channels.insert(2, make_channel(2, 1, "Child"));
        state.users.insert(10, make_user(10, 1, "Alice"));

        assert_eq!(state.channel(1).unwrap().name, "Root");
        assert!(state.channel(99).is_none());
        assert_eq!(state.user(10).unwrap().nickname, "Alice");
        assert!(state.user(99).is_none());
    }

    #[test]
    fn users_in_channel() {
        let mut state = ServerState::default();
        state.users.insert(1, make_user(1, 10, "A"));
        state.users.insert(2, make_user(2, 10, "B"));
        state.users.insert(3, make_user(3, 20, "C"));
        assert_eq!(state.users_in_channel(10).len(), 2);
        assert_eq!(state.users_in_channel(20).len(), 1);
        assert_eq!(state.users_in_channel(99).len(), 0);
    }

    #[test]
    fn root_and_child_channels() {
        let mut state = ServerState::default();
        state.channels.insert(1, make_channel(1, 0, "Root1"));
        state.channels.insert(2, make_channel(2, 0, "Root2"));
        state.channels.insert(3, make_channel(3, 1, "Child"));
        assert_eq!(state.root_channels().len(), 2);
        assert_eq!(state.child_channels(1).len(), 1);
        assert_eq!(state.child_channels(2).len(), 0);
    }

    #[test]
    fn find_by_name() {
        let mut state = ServerState::default();
        state.channels.insert(1, make_channel(1, 0, "Lobby"));
        state.users.insert(1, make_user(1, 1, "Bob"));
        assert_eq!(state.find_channel_by_name("Lobby").unwrap().id, 1);
        assert!(state.find_channel_by_name("Nope").is_none());
        assert_eq!(state.find_user_by_name("Bob").unwrap().id, 1);
        assert!(state.find_user_by_name("Nope").is_none());
    }
}

/// Channel group information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelGroup {
    /// Group ID
    pub id: u64,
    /// Group name
    pub name: String,
    /// Group type
    pub group_type: u8,
    /// Icon ID
    pub icon_id: i64,
    /// Is permanent
    pub is_permanent: bool,
    /// Sort order
    pub sort_id: i32,
}
