//! Audio configuration

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Audio configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Sample rate (usually 48000 for Opus)
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo)
    pub channels: u16,
    /// Frame size in milliseconds (20ms is standard for TS3)
    pub frame_size_ms: u32,
    /// Opus bitrate (usually 32000-96000)
    pub bitrate: u32,
    /// Opus application mode
    pub opus_application: OpusApplication,
    /// Voice activity detection enabled
    pub vad_enabled: bool,
    /// VAD threshold (0.0-1.0)
    pub vad_threshold: f32,
    /// Automatic gain control
    pub agc_enabled: bool,
    /// Target gain for AGC (dB)
    pub agc_target: f32,
    /// Noise suppression enabled
    pub noise_suppression: bool,
    /// Echo cancellation enabled (planned)
    pub echo_cancellation: bool,
    /// Input device name (None for default)
    pub input_device: Option<String>,
    /// Output device name (None for default)
    pub output_device: Option<String>,
    /// Input volume (0.0-2.0, 1.0 = normal)
    pub input_volume: f32,
    /// Output volume (0.0-2.0, 1.0 = normal)
    pub output_volume: f32,
    /// Playback buffer size in milliseconds
    pub playback_buffer_ms: u32,
    /// Per-user decoder cache limits
    #[serde(default)]
    pub decoder_pool: DecoderPoolConfig,
    /// Incoming voice jitter buffer
    #[serde(default)]
    pub jitter_buffer: JitterBufferConfig,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 1,
            frame_size_ms: 20,
            bitrate: 64000,
            opus_application: OpusApplication::Voip,
            vad_enabled: true,
            vad_threshold: 0.3,
            agc_enabled: true,
            agc_target: -18.0,
            noise_suppression: true,
            echo_cancellation: false, // Not implemented yet
            input_device: None,
            output_device: None,
            input_volume: 1.0,
            output_volume: 1.0,
            playback_buffer_ms: 60,
            decoder_pool: DecoderPoolConfig::default(),
            jitter_buffer: JitterBufferConfig::default(),
        }
    }
}

impl AudioConfig {
    /// Create a config optimized for music
    pub fn music() -> Self {
        Self {
            channels: 2,
            bitrate: 96000,
            opus_application: OpusApplication::Audio,
            vad_enabled: false,
            noise_suppression: false,
            ..Default::default()
        }
    }

    /// Create a config optimized for low latency
    pub fn low_latency() -> Self {
        Self {
            frame_size_ms: 10,
            playback_buffer_ms: 40,
            ..Default::default()
        }
    }

    /// Get the frame size in samples
    pub fn frame_size_samples(&self) -> usize {
        (self.sample_rate * self.frame_size_ms / 1000) as usize
    }

    /// Get the playback buffer size in samples
    pub fn playback_buffer_samples(&self) -> usize {
        (self.sample_rate * self.playback_buffer_ms / 1000) as usize
    }
}

/// Fixed-delay jitter buffer configuration for incoming voice.
///
/// Real-time voice is latency sensitive, so the target delay is fixed: it does
/// not grow with observed jitter. Packets that arrive after their play time
/// are dropped instead of delaying the stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JitterBufferConfig {
    /// Fixed delay between first packet arrival and first playback (ms).
    pub target_delay_ms: u32,
    /// Duration of one voice frame (ms), used to schedule consecutive packets.
    pub frame_duration_ms: u32,
    /// Maximum consecutive PLC frames before waiting for a real packet.
    pub max_plc_frames: u32,
    /// Upper bound on buffered packets per user; excess is dropped oldest.
    pub max_buffered_frames: usize,
}

impl Default for JitterBufferConfig {
    fn default() -> Self {
        Self {
            target_delay_ms: 60,
            frame_duration_ms: 20,
            max_plc_frames: 3,
            max_buffered_frames: 8,
        }
    }
}

impl JitterBufferConfig {
    /// Target delay as a `Duration`.
    pub fn target_delay(&self) -> Duration {
        Duration::from_millis(self.target_delay_ms as u64)
    }

    /// Frame duration as a `Duration`.
    pub fn frame_duration(&self) -> Duration {
        Duration::from_millis(self.frame_duration_ms.max(1) as u64)
    }
}

/// Per-user decoder cache configuration.
///
/// Every remote speaker needs a separate Opus decoder. A long-running session
/// on a busy server must both release decoders for users that left and bound
/// the cache for users that went quiet, otherwise memory grows for the whole
/// session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecoderPoolConfig {
    /// Release a decoder after this long without audio.
    ///
    /// `None` keeps quiet users until they leave or `max_decoders` forces an
    /// eviction. Defaults to 5 minutes. Serialized as milliseconds.
    #[serde(with = "duration_ms", default = "duration_ms::default_timeout")]
    pub idle_timeout: Option<Duration>,
    /// Maximum number of cached decoders; the least recently used one is
    /// evicted first. Defaults to 512; values below 1 are treated as 1.
    #[serde(default = "default_max_decoders")]
    pub max_decoders: usize,
}

impl Default for DecoderPoolConfig {
    fn default() -> Self {
        Self {
            idle_timeout: duration_ms::default_timeout(),
            max_decoders: default_max_decoders(),
        }
    }
}

fn default_max_decoders() -> usize {
    512
}

/// Serialize `Option<Duration>` as optional milliseconds, which is friendlier
/// in JSON config files than serde's default `{secs, nanos}` struct.
mod duration_ms {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn default_timeout() -> Option<Duration> {
        Some(Duration::from_secs(5 * 60))
    }

    pub fn serialize<S: Serializer>(
        value: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .map(|duration| duration.as_millis() as u64)
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        Option::<u64>::deserialize(deserializer).map(|millis| millis.map(Duration::from_millis))
    }
}

/// Opus application mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpusApplication {
    /// Optimized for voice
    Voip,
    /// Optimized for music/audio
    Audio,
    /// Restricted low delay mode
    LowDelay,
}

impl OpusApplication {
    /// Convert to opus library constant
    pub fn to_opus_const(&self) -> i32 {
        match self {
            Self::Voip => 2048,     // OPUS_APPLICATION_VOIP
            Self::Audio => 2049,    // OPUS_APPLICATION_AUDIO
            Self::LowDelay => 2051, // OPUS_APPLICATION_RESTRICTED_LOWDELAY
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let cfg = AudioConfig::default();
        assert_eq!(cfg.sample_rate, 48000);
        assert_eq!(cfg.channels, 1);
        assert_eq!(cfg.frame_size_ms, 20);
        assert_eq!(cfg.bitrate, 64000);
        assert_eq!(cfg.opus_application, OpusApplication::Voip);
        assert!(cfg.vad_enabled);
        assert!(cfg.noise_suppression);
        assert!(!cfg.echo_cancellation);
        assert_eq!(cfg.input_volume, 1.0);
        assert_eq!(cfg.output_volume, 1.0);
    }

    #[test]
    fn music_preset() {
        let cfg = AudioConfig::music();
        assert_eq!(cfg.channels, 2);
        assert_eq!(cfg.bitrate, 96000);
        assert_eq!(cfg.opus_application, OpusApplication::Audio);
        assert!(!cfg.vad_enabled);
        assert!(!cfg.noise_suppression);
        // inherited defaults
        assert_eq!(cfg.sample_rate, 48000);
    }

    #[test]
    fn low_latency_preset() {
        let cfg = AudioConfig::low_latency();
        assert_eq!(cfg.frame_size_ms, 10);
        assert_eq!(cfg.playback_buffer_ms, 40);
        // inherited defaults
        assert_eq!(cfg.sample_rate, 48000);
        assert_eq!(cfg.channels, 1);
    }

    #[test]
    fn frame_size_samples_default() {
        let cfg = AudioConfig::default();
        // 48000 * 20 / 1000 = 960
        assert_eq!(cfg.frame_size_samples(), 960);
    }

    #[test]
    fn frame_size_samples_low_latency() {
        let cfg = AudioConfig::low_latency();
        // 48000 * 10 / 1000 = 480
        assert_eq!(cfg.frame_size_samples(), 480);
    }

    #[test]
    fn playback_buffer_samples() {
        let cfg = AudioConfig::default();
        // 48000 * 60 / 1000 = 2880
        assert_eq!(cfg.playback_buffer_samples(), 2880);
    }

    #[test]
    fn decoder_pool_defaults() {
        let cfg = AudioConfig::default();
        assert_eq!(
            cfg.decoder_pool.idle_timeout,
            Some(Duration::from_secs(5 * 60))
        );
        assert_eq!(cfg.decoder_pool.max_decoders, 512);
    }

    #[test]
    fn opus_application_constants() {
        assert_eq!(OpusApplication::Voip.to_opus_const(), 2048);
        assert_eq!(OpusApplication::Audio.to_opus_const(), 2049);
        assert_eq!(OpusApplication::LowDelay.to_opus_const(), 2051);
    }
}
