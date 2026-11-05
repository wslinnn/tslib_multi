//! Audio error types

use thiserror::Error;

/// Result type for audio operations
pub type Result<T> = std::result::Result<T, AudioError>;

/// Audio-related errors
#[derive(Error, Debug)]
pub enum AudioError {
    /// Failed to initialize audio device
    #[error("Failed to initialize audio device: {0}")]
    DeviceInit(String),

    /// Failed to open capture device
    #[error("Failed to open capture device: {0}")]
    CaptureOpen(String),

    /// Failed to open playback device
    #[error("Failed to open playback device: {0}")]
    PlaybackOpen(String),

    /// Codec error
    #[error("Codec error: {0}")]
    Codec(String),

    /// Incoming codec is not supported by this decoder path
    #[error("Unsupported audio codec: {0}")]
    UnsupportedCodec(u8),

    /// Encoding error
    #[error("Encoding error: {0}")]
    Encode(String),

    /// Decoding error
    #[error("Decoding error: {0}")]
    Decode(String),

    /// Resampling error
    #[error("Resampling error: {0}")]
    Resample(String),

    /// No audio device available
    #[error("No audio device available")]
    NoDevice,

    /// Device not found
    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    /// Audio buffer overflow
    #[error("Audio buffer overflow")]
    BufferOverflow,

    /// Audio buffer underflow
    #[error("Audio buffer underflow")]
    BufferUnderflow,

    /// Invalid configuration
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// Internal error
    #[error("Internal audio error: {0}")]
    Internal(String),
}
