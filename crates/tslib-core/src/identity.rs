//! Identity management for TeamSpeak 3
//!
//! TeamSpeak identities are based on ECDH (Elliptic Curve Diffie-Hellman)
//! and include a security level that must meet server requirements.

use crate::error::{IdentityError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tsproto_types::crypto::EccKeyPrivP256;

// Re-export tsclientlib's identity for internal use
pub use tsclientlib::Identity as TsIdentity;

/// A TeamSpeak 3 identity wrapper
///
/// Wraps tsclientlib's Identity with additional convenience methods.
#[derive(Clone)]
pub struct Identity {
    /// The underlying tsclientlib identity
    inner: TsIdentity,
    /// Optional nickname associated with this identity
    nickname: Option<String>,
}

impl Identity {
    /// Create a new random identity
    ///
    /// The identity starts at security level 0 and should be improved
    /// before connecting to servers with security requirements.
    pub fn create() -> Result<Self> {
        let inner = TsIdentity::create();
        Ok(Self {
            inner,
            nickname: None,
        })
    }

    /// Create an identity from the underlying tsclientlib identity
    pub fn from_ts_identity(inner: TsIdentity) -> Self {
        Self {
            inner,
            nickname: None,
        }
    }

    /// Get the underlying tsclientlib identity
    pub fn as_ts_identity(&self) -> &TsIdentity {
        &self.inner
    }

    /// Get a clone of the underlying tsclientlib identity
    pub fn to_ts_identity(&self) -> TsIdentity {
        self.inner.clone()
    }

    /// Load an identity from a file
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| IdentityError::ImportFailed(e.to_string()))?;

        Self::from_string(&content)
    }

    /// Save the identity to a file
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let content = self.export_string()?;
        std::fs::write(path.as_ref(), content)
            .map_err(|e| IdentityError::ExportFailed(e.to_string()))?;
        Ok(())
    }

    /// Import an identity from a base64-encoded string
    pub fn from_string(data: &str) -> Result<Self> {
        // Try to parse as our JSON format first
        if let Ok(stored) = serde_json::from_str::<StoredIdentity>(data) {
            let inner = TsIdentity::new_from_str(&stored.key)
                .map_err(|e| IdentityError::ImportFailed(e.to_string()))?;
            return Ok(Self {
                inner,
                nickname: stored.nickname,
            });
        }

        // Official TeamSpeak client export (ts.ini or a bare obfuscated
        // `<offset>V<key>` value) — must run before the short-format parse
        // so the obfuscated blob can never be mistaken for a plain key
        if let Ok(id) = Self::from_team_speak_ini(data) {
            return Ok(id);
        }

        // Try to parse as raw tsclientlib format
        let inner = TsIdentity::new_from_str(data.trim())
            .map_err(|e| IdentityError::ImportFailed(e.to_string()))?;

        Ok(Self {
            inner,
            nickname: None,
        })
    }

    /// Import from an official TeamSpeak client identity export (`ts.ini`)
    /// or a bare `<offset>V<obfuscated key>` value. The ini `nickname` is
    /// restored with its `\xNN` escapes resolved.
    pub fn from_team_speak_ini(data: &str) -> Result<Self> {
        let mut value: Option<String> = None;
        let mut nickname: Option<String> = None;
        for line in data.trim().lines() {
            let line = line.trim();
            if line.starts_with(';') || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let Some((key, val)) = line.split_once('=') else { continue };
            // Strip inline ini comments, quotes and whitespace
            let val = val.split(';').next().unwrap_or(val);
            let val = val.split('#').next().unwrap_or(val);
            let val = val.trim().trim_matches('"').trim_matches('\'').trim();
            match key.trim().to_ascii_lowercase().as_str() {
                "identity" => value = Some(val.to_string()),
                "nickname" => nickname = Some(unescape_team_speak_text(val)),
                _ => {}
            }
        }

        // No ini entry: accept a bare `<offset>V<obfuscated>` value
        let value = match value {
            Some(v) if !v.is_empty() => v,
            _ => {
                let raw: String = data
                    .chars()
                    .filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
                    .collect();
                if raw.is_empty() || raw.contains('=') {
                    return Err(IdentityError::ImportFailed(
                        "no identity entry found".into(),
                    ));
                }
                raw
            }
        };

        let separator = value
            .find('V')
            .ok_or_else(|| IdentityError::ImportFailed("missing offset separator".into()))?;
        if separator == 0 || separator == value.len() - 1 {
            return Err(IdentityError::ImportFailed("missing key after offset".into()));
        }
        let offset: u64 = value[..separator]
            .parse()
            .map_err(|e| IdentityError::ImportFailed(format!("invalid offset: {e}")))?;
        let encoded: String = value[separator + 1..]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();

        let key = EccKeyPrivP256::from_ts_obfuscated(&encoded)
            .map_err(|e| IdentityError::ImportFailed(format!("obfuscated key: {e}")))?;
        Ok(Self {
            inner: TsIdentity::new(key, offset),
            nickname,
        })
    }

    /// Export as an official TeamSpeak client identity file (`ts.ini`),
    /// importable by desktop clients. `id_label` fills the ini `id=` field.
    pub fn export_team_speak_ini(&self, id_label: &str) -> Result<String> {
        let obfuscated = self.inner.key().to_ts_obfuscated();
        let value = format!("{}V{}", self.inner.counter(), obfuscated);
        let nickname = self.nickname.as_deref().unwrap_or_default();
        Ok(format!(
            "[Identity]\r\nid={}\r\nidentity=\"{}\"\r\nnickname={}\r\nphonetic_nickname=\r\n",
            id_label,
            value,
            escape_team_speak_text(nickname),
        ))
    }

    /// Export the identity to a string
    pub fn export_string(&self) -> Result<String> {
        // Export the key in tsclientlib format
        let key = self.export_key_string();
        let stored = StoredIdentity {
            key,
            nickname: self.nickname.clone(),
        };
        serde_json::to_string_pretty(&stored)
            .map_err(|e| IdentityError::ExportFailed(e.to_string()).into())
    }

    /// Export just the key string in tsclientlib format
    fn export_key_string(&self) -> String {
        // Format: counterVbase64(private_key_bytes)
        use base64::Engine;
        let key_bytes = self.inner.key().to_short();
        let counter = self.inner.counter();
        let key_b64 = base64::engine::general_purpose::STANDARD.encode(&key_bytes);
        format!("{}V{}", counter, key_b64)
    }

    /// Get the unique identifier (public key hash)
    pub fn unique_id(&self) -> String {
        self.inner.key().to_pub().get_uid()
    }

    /// Get the current security level
    pub fn security_level(&self) -> u8 {
        self.inner.level()
    }

    /// Get the key counter (offset)
    pub fn key_offset(&self) -> u64 {
        self.inner.counter()
    }

    /// Get the associated nickname, if any
    pub fn nickname(&self) -> Option<&str> {
        self.nickname.as_deref()
    }

    /// Set the nickname for this identity
    pub fn set_nickname(&mut self, nickname: impl Into<String>) {
        self.nickname = Some(nickname.into());
    }

    /// Improve the identity's security level synchronously
    ///
    /// This is a CPU-intensive operation that finds a key offset
    /// which produces a hash with enough leading zeros.
    ///
    /// # Arguments
    /// * `target_level` - The desired security level (8-32 typically)
    pub fn improve(&mut self, target_level: u8) -> Result<()> {
        if target_level <= self.inner.level() {
            return Ok(());
        }

        self.inner.upgrade_level(target_level);
        Ok(())
    }

    /// Improve the identity asynchronously
    ///
    /// Runs the CPU-intensive work in a blocking task pool.
    pub async fn improve_async(&mut self, target_level: u8) -> Result<()> {
        if target_level <= self.inner.level() {
            return Ok(());
        }

        let mut inner = self.inner.clone();

        let result = tokio::task::spawn_blocking(move || {
            inner.upgrade_level(target_level);
            inner
        })
        .await
        .map_err(|e| IdentityError::ImproveFailed(e.to_string()))?;

        self.inner = result;
        Ok(())
    }
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("unique_id", &self.unique_id())
            .field("security_level", &self.security_level())
            .field("nickname", &self.nickname)
            .finish_non_exhaustive()
    }
}

/// Stored identity format for serialization
#[derive(Debug, Serialize, Deserialize)]
struct StoredIdentity {
    /// The identity key in tsclientlib format
    key: String,
    /// Optional nickname
    #[serde(skip_serializing_if = "Option::is_none")]
    nickname: Option<String>,
}

/// Resolve `\xNN` escapes (the official client escapes every non-ASCII byte
/// and backslash this way) into UTF-8 text.
fn unescape_team_speak_text(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'x') {
            if let Some(hex) = bytes.get(i + 2..i + 4) {
                if let Ok(b) = u8::from_str_radix(std::str::from_utf8(hex).unwrap_or(""), 16) {
                    out.push(b);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Escape into the official `\xNN` form: non-ASCII bytes, backslash and
/// double quote become `\xNN`, other printable ASCII passes through.
fn escape_team_speak_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.as_bytes() {
        match b {
            b'\\' => out.push_str("\\x5c"),
            b'"' => out.push_str("\\x22"),
            0x20..=0x7e => out.push(*b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_identity() {
        let identity = Identity::create().unwrap();
        assert!(!identity.unique_id().is_empty());
        assert!(identity.security_level() < 100);
    }

    #[test]
    fn test_serialize_identity() {
        let identity = Identity::create().unwrap();
        let json = identity.export_string().unwrap();
        let loaded = Identity::from_string(&json).unwrap();
        assert_eq!(identity.unique_id(), loaded.unique_id());
    }

    #[test]
    fn test_team_speak_ini_roundtrip() {
        let mut identity = Identity::create().unwrap();
        identity.set_nickname("机器人");
        let ini = identity.export_team_speak_ini("Default").unwrap();
        assert!(ini.contains("identity=\""));
        assert!(ini.contains("nickname=\\x"));
        let loaded = Identity::from_team_speak_ini(&ini).unwrap();
        assert_eq!(identity.unique_id(), loaded.unique_id());
        assert_eq!(identity.key_offset(), loaded.key_offset());
        assert_eq!(loaded.nickname().as_deref(), Some("机器人"));
    }

    #[test]
    fn test_from_string_accepts_team_speak_ini() {
        let mut identity = Identity::create().unwrap();
        identity.set_nickname("tester");
        let ini = identity.export_team_speak_ini("Default").unwrap();
        let loaded = Identity::from_string(&ini).unwrap();
        assert_eq!(identity.unique_id(), loaded.unique_id());
        assert_eq!(loaded.nickname().as_deref(), Some("tester"));
    }
}
