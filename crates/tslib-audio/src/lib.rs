//! # tslib-audio
//!
//! Audio module for tslib providing:
//! - Microphone capture
//! - Speaker playback
//! - Opus encoding/decoding
//! - Audio mixing
//! - Software echo cancellation (speexdsp-based)
//!
//! ## Example
//!
//! ```rust,no_run
//! use tslib_audio::{AudioManager, AudioConfig};
//!
//! # async fn example() -> anyhow::Result<()> {
//! let config = AudioConfig::default();
//! let audio = AudioManager::new(config)?;
//!
//! // Start capture — returns a receiver for encoded packets
//! let mut rx = audio.start_capture().await?;
//!
//! // Get encoded audio for transmission
//! while let Some(packet) = rx.recv().await {
//!     // Send packet to server...
//! }
//! # Ok(())
//! # }
//! ```

pub mod capture;
pub mod codec;
pub mod config;
pub mod echo;
pub mod error;
pub mod jitter;
pub mod manager;
pub mod mixer;
pub mod playback;

// Re-exports
pub use capture::{CaptureDevice, CaptureStats, SampleBufferPool};
pub use codec::{Decoder, Encoder, OpusCodec};
pub use config::{AudioConfig, DecoderPoolConfig, JitterBufferConfig};
pub use echo::{EchoEngine, SpeexEchoEngine};
pub use error::{AudioError, Result};
pub use jitter::{JitterBuffer, JitterOutput, JitterStats};
pub use manager::{AudioManager, AudioStats};
pub use playback::{PlaybackDevice, PlaybackStats};
