//! Client configuration

use crate::error::{Error, Result};
use crate::identity::Identity;
use serde::Serialize;
use std::time::Duration;

/// Client configuration builder
#[derive(Debug, Clone)]
pub struct ClientConfigBuilder {
    address: Option<String>,
    identity: Option<Identity>,
    nickname: Option<String>,
    password: Option<String>,
    channel: Option<String>,
    channel_password: Option<String>,
    hardware_id: Option<String>,
    connect_timeout: Duration,
    auto_reconnect: bool,
    reconnect_delay: Duration,
    max_reconnect_attempts: u32,
}

impl Default for ClientConfigBuilder {
    fn default() -> Self {
        Self {
            address: None,
            identity: None,
            nickname: None,
            password: None,
            channel: None,
            channel_password: None,
            hardware_id: None,
            connect_timeout: Duration::from_secs(10),
            auto_reconnect: true,
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_attempts: 5,
        }
    }
}

impl ClientConfigBuilder {
    /// Set the server address (host:port or just host for default port 9987)
    pub fn address(mut self, address: impl Into<String>) -> Self {
        self.address = Some(address.into());
        self
    }

    /// Set the identity to use for authentication
    pub fn identity(mut self, identity: Identity) -> Self {
        self.identity = Some(identity);
        self
    }

    /// Set the nickname to display on the server
    pub fn nickname(mut self, nickname: impl Into<String>) -> Self {
        self.nickname = Some(nickname.into());
        self
    }

    /// Set the server password (if required)
    pub fn password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Set the default channel to join
    pub fn channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = Some(channel.into());
        self
    }

    /// Set the channel password (if required)
    pub fn channel_password(mut self, password: impl Into<String>) -> Self {
        self.channel_password = Some(password.into());
        self
    }

    /// Set a custom hardware ID
    pub fn hardware_id(mut self, hwid: impl Into<String>) -> Self {
        self.hardware_id = Some(hwid.into());
        self
    }

    /// Set the connection timeout
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Enable or disable automatic reconnection
    pub fn auto_reconnect(mut self, enabled: bool) -> Self {
        self.auto_reconnect = enabled;
        self
    }

    /// Set the delay between reconnection attempts
    pub fn reconnect_delay(mut self, delay: Duration) -> Self {
        self.reconnect_delay = delay;
        self
    }

    /// Set the maximum number of reconnection attempts
    pub fn max_reconnect_attempts(mut self, max: u32) -> Self {
        self.max_reconnect_attempts = max;
        self
    }

    /// Build the configuration
    pub fn build(self) -> Result<ClientConfig> {
        let address = self
            .address
            .ok_or_else(|| Error::Config("Server address is required".into()))?;

        let identity = self
            .identity
            .ok_or_else(|| Error::Config("Identity is required".into()))?;

        let nickname = self.nickname.unwrap_or_else(|| {
            identity
                .nickname()
                .map(String::from)
                .unwrap_or_else(|| "TsLibUser".to_string())
        });

        Ok(ClientConfig {
            address: Self::normalize_address(&address)?,
            identity,
            nickname,
            password: self.password,
            channel: self.channel,
            channel_password: self.channel_password,
            hardware_id: self.hardware_id.unwrap_or_else(Self::generate_hardware_id),
            connect_timeout: self.connect_timeout,
            auto_reconnect: self.auto_reconnect,
            reconnect_delay: self.reconnect_delay,
            max_reconnect_attempts: self.max_reconnect_attempts,
        })
    }

    fn normalize_address(address: &str) -> Result<String> {
        if address.contains(':') {
            Ok(address.to_string())
        } else {
            Ok(format!("{}:9987", address))
        }
    }

    fn generate_hardware_id() -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let bytes: [u8; 16] = rng.gen();
        hex::encode(bytes)
    }
}

/// Client configuration
#[derive(Debug, Clone, Serialize)]
pub struct ClientConfig {
    /// Server address (host:port)
    pub address: String,
    /// Identity for authentication
    #[serde(skip_serializing)]
    pub identity: Identity,
    /// Display nickname
    pub nickname: String,
    /// Server password (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Default channel to join
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Channel password (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_password: Option<String>,
    /// Hardware ID
    pub hardware_id: String,
    /// Connection timeout
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Duration,
    /// Auto-reconnect on disconnect
    pub auto_reconnect: bool,
    /// Delay between reconnect attempts
    #[serde(with = "humantime_serde")]
    pub reconnect_delay: Duration,
    /// Maximum reconnect attempts
    pub max_reconnect_attempts: u32,
}

impl ClientConfig {
    /// Create a new configuration builder
    pub fn builder() -> ClientConfigBuilder {
        ClientConfigBuilder::default()
    }

    /// Get the host part of the address
    pub fn host(&self) -> &str {
        self.address.split(':').next().unwrap_or(&self.address)
    }

    /// Get the port from the address
    pub fn port(&self) -> u16 {
        self.address
            .split(':')
            .nth(1)
            .and_then(|p| p.parse().ok())
            .unwrap_or(9987)
    }
}

/// File transfer limits and timeouts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTransferConfig {
    /// Maximum size of a file downloaded into memory. `None` disables the
    /// limit and should only be used when the caller can bound memory itself.
    pub max_memory_download: Option<u64>,
    /// Maximum time without progress while reading or writing a transfer.
    /// `None` disables the idle timeout.
    pub idle_timeout: Option<Duration>,
    /// Chunk size used for streaming reads and writes.
    pub chunk_size: usize,
}

impl Default for FileTransferConfig {
    fn default() -> Self {
        Self {
            max_memory_download: Some(64 * 1024 * 1024),
            idle_timeout: Some(Duration::from_secs(30)),
            chunk_size: 64 * 1024,
        }
    }
}

impl FileTransferConfig {
    /// Create a configuration with no memory limit, no idle timeout, and the
    /// default chunk size.
    pub fn unlimited() -> Self {
        Self {
            max_memory_download: None,
            idle_timeout: None,
            chunk_size: Self::default().chunk_size,
        }
    }
}

// Helper module for Duration serialization
#[allow(dead_code)]
mod humantime_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{}s", duration.as_secs()))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let secs = s
            .trim_end_matches('s')
            .parse::<u64>()
            .map_err(serde::de::Error::custom)?;
        Ok(Duration::from_secs(secs))
    }
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    fn make_config(address: &str) -> Result<ClientConfig> {
        ClientConfigBuilder::default()
            .address(address)
            .identity(Identity::create().unwrap())
            .build()
    }

    #[test]
    fn normalize_address_adds_default_port() {
        let config = make_config("localhost").unwrap();
        assert_eq!(config.address, "localhost:9987");
    }

    #[test]
    fn normalize_address_preserves_explicit_port() {
        let config = make_config("localhost:1234").unwrap();
        assert_eq!(config.address, "localhost:1234");
    }

    #[test]
    fn host_returns_host_part() {
        let config = make_config("example.com:9987").unwrap();
        assert_eq!(config.host(), "example.com");
    }

    #[test]
    fn port_parses_from_address() {
        let config = make_config("example.com:1234").unwrap();
        assert_eq!(config.port(), 1234);
    }

    #[test]
    fn port_defaults_to_9987() {
        let mut config = make_config("example.com").unwrap();
        config.address = "example.com".to_string();
        assert_eq!(config.port(), 9987);
    }

    #[test]
    fn build_fails_without_address() {
        let result = ClientConfigBuilder::default()
            .identity(Identity::create().unwrap())
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_fails_without_identity() {
        let result = ClientConfigBuilder::default()
            .address("localhost")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn default_nickname_is_tslib_user() {
        let config = make_config("localhost").unwrap();
        assert_eq!(config.nickname, "TsLibUser");
    }

    #[test]
    fn custom_nickname_is_used() {
        let config = ClientConfigBuilder::default()
            .address("localhost")
            .identity(Identity::create().unwrap())
            .nickname("Bot")
            .build()
            .unwrap();
        assert_eq!(config.nickname, "Bot");
    }

    #[test]
    fn hardware_id_is_32_hex_chars() {
        let config = make_config("localhost").unwrap();
        assert_eq!(config.hardware_id.len(), 32);
        assert!(config.hardware_id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn builder_defaults() {
        let builder = ClientConfigBuilder::default();
        assert_eq!(builder.connect_timeout, Duration::from_secs(10));
        assert!(builder.auto_reconnect);
        assert_eq!(builder.reconnect_delay, Duration::from_secs(5));
        assert_eq!(builder.max_reconnect_attempts, 5);
    }
}
