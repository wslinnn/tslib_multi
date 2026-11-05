//! # tslib-core
//!
//! Core TeamSpeak 3 client library providing connection management,
//! identity handling, and protocol abstraction.
//!
//! ## Features
//!
//! - Connection to TeamSpeak 3 servers
//! - Identity creation and management
//! - Event-driven architecture
//! - Async/await support
//!
//! ## Example
//!
//! ```rust,no_run
//! use tslib_core::{Client, ClientConfig, Identity};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let identity = Identity::create()?;
//!
//!     let config = ClientConfig::builder()
//!         .address("localhost:9987")
//!         .identity(identity)
//!         .nickname("TsLibBot")
//!         .build()?;
//!
//!     let mut client = Client::connect(config)?;
//!
//!     // Handle events...
//!     client.disconnect()?;
//!     Ok(())
//! }
//! ```

pub mod actor;
pub mod client;
pub mod config;
pub mod connection;
pub mod error;
pub mod events;
pub mod ids;
pub mod identity;
pub mod permissions;
pub mod state;

// Re-exports
pub use actor::{ActorEventStream, ActorHandle};
pub use client::{
    Client, ClientDiagnostics, FileEntry, NetworkStats, PumpStats, ServerView,
};
pub use config::{ClientConfig, FileTransferConfig};
pub use connection::{Connection, ConnectionState};
pub use error::{Error, Result};
pub use events::{AudioCodec, Event, EventHandler};
pub use identity::Identity;
pub use ids::{
    ChannelGroupId, ChannelId, ClientId, DatabaseId, IconId, PermissionId, ServerGroupId,
};
pub use permissions::*;
pub use state::{Channel, MaxClientsKind, PermissionHints, ServerState, User};
pub use tsproto_types::versions::Version as ClientVersion;
