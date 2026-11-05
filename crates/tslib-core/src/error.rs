//! Error types for tslib-core

use thiserror::Error;

/// Result type alias for tslib operations
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for tslib-core
#[derive(Error, Debug)]
pub enum Error {
    /// Connection-related errors
    #[error("Connection error: {0}")]
    Connection(#[from] ConnectionError),

    /// Identity-related errors
    #[error("Identity error: {0}")]
    Identity(#[from] IdentityError),

    /// Protocol errors from the server
    #[error("Server error: {code} - {message}")]
    Server { code: u32, message: String },

    /// Permission denied
    #[error("Permission denied: {permission}")]
    PermissionDenied { permission: String },

    /// Channel-related errors
    #[error("Channel error: {0}")]
    Channel(String),

    /// Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),

    /// Invalid argument provided by the caller
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    /// Timeout errors
    #[error("Operation timed out")]
    Timeout,

    /// Internal errors
    #[error("Internal error: {0}")]
    Internal(String),

    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Connection-specific errors
#[derive(Error, Debug)]
pub enum ConnectionError {
    #[error("Failed to connect to server: {0}")]
    ConnectFailed(String),

    #[error("Connection lost: {0}")]
    ConnectionLost(String),

    #[error("Already connected")]
    AlreadyConnected,

    #[error("Not connected")]
    NotConnected,

    #[error("Invalid server address: {0}")]
    InvalidAddress(String),

    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("Server rejected connection: {0}")]
    Rejected(String),

    #[error("DNS resolution failed: {0}")]
    DnsError(String),
}

/// Identity-specific errors
#[derive(Error, Debug)]
pub enum IdentityError {
    #[error("Failed to create identity: {0}")]
    CreationFailed(String),

    #[error("Invalid identity format")]
    InvalidFormat,

    #[error("Failed to improve identity: {0}")]
    ImproveFailed(String),

    #[error("Security level too low: required {required}, current {current}")]
    SecurityLevelTooLow { required: u8, current: u8 },

    #[error("Failed to export identity: {0}")]
    ExportFailed(String),

    #[error("Failed to import identity: {0}")]
    ImportFailed(String),
}

impl Error {
    /// Create a server error from code and message
    pub fn server(code: u32, message: impl Into<String>) -> Self {
        Self::Server {
            code,
            message: message.into(),
        }
    }

    /// Create a permission denied error
    pub fn permission_denied(permission: impl Into<String>) -> Self {
        Self::PermissionDenied {
            permission: permission.into(),
        }
    }

    /// Check if this is a recoverable error
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Error::Timeout | Error::Connection(ConnectionError::ConnectionLost(_))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_error_construction() {
        let err = Error::server(256, "not found");
        match err {
            Error::Server { code, message } => {
                assert_eq!(code, 256);
                assert_eq!(message, "not found");
            }
            _ => panic!("expected Server variant"),
        }
    }

    #[test]
    fn permission_denied_construction() {
        let err = Error::permission_denied("b_client_kick");
        match err {
            Error::PermissionDenied { permission } => {
                assert_eq!(permission, "b_client_kick");
            }
            _ => panic!("expected PermissionDenied variant"),
        }
    }

    #[test]
    fn timeout_is_recoverable() {
        assert!(Error::Timeout.is_recoverable());
    }

    #[test]
    fn connection_lost_is_recoverable() {
        let err = Error::Connection(ConnectionError::ConnectionLost("reset".into()));
        assert!(err.is_recoverable());
    }

    #[test]
    fn config_error_is_not_recoverable() {
        let err = Error::Config("bad".into());
        assert!(!err.is_recoverable());
    }

    #[test]
    fn server_error_is_not_recoverable() {
        assert!(!Error::server(1, "x").is_recoverable());
    }

    #[test]
    fn not_connected_is_not_recoverable() {
        let err = Error::Connection(ConnectionError::NotConnected);
        assert!(!err.is_recoverable());
    }

    #[test]
    fn error_display_server() {
        let err = Error::server(512, "banned");
        assert_eq!(format!("{}", err), "Server error: 512 - banned");
    }

    #[test]
    fn error_display_permission() {
        let err = Error::permission_denied("kick");
        assert_eq!(format!("{}", err), "Permission denied: kick");
    }

    #[test]
    fn error_display_timeout() {
        assert_eq!(format!("{}", Error::Timeout), "Operation timed out");
    }
}
