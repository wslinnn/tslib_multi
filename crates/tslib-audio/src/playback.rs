//! Audio playback (speaker output)

use crate::capture::DeviceInfo;
use crate::config::AudioConfig;
use crate::error::{AudioError, Result};
use crate::mixer::AudioMixer;
use crate::resample::{map_channels, LinearResampler};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, error};

/// Audio playback device
pub struct PlaybackDevice {
    config: AudioConfig,
    /// The cpal stream handle
    stream: Option<cpal::Stream>,
    /// Shared mixer for all user audio
    mixer: Arc<Mutex<AudioMixer>>,
    /// Shared playback state
    state: Arc<PlaybackState>,
    /// Recent speaker output, used as the echo-cancellation far-end reference
    reference: Arc<Mutex<VecDeque<i16>>>,
}

/// Counters for the realtime playback path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlaybackStats {
    /// Output blocks replaced with silence because the mixer was locked.
    pub dropped_blocks: u64,
    /// Output blocks that contained no user audio (buffer underrun).
    pub underruns: u64,
}

/// Shared state between playback thread and main thread
struct PlaybackState {
    /// Is playback active
    is_active: AtomicBool,
    /// Output volume (scaled by 100 for atomic ops)
    volume: AtomicU64,
    /// Blocks dropped due to mixer lock contention
    dropped_blocks: AtomicU64,
}

impl PlaybackState {
    fn new(volume: f32) -> Self {
        Self {
            is_active: AtomicBool::new(false),
            volume: AtomicU64::new((volume * 100.0) as u64),
            dropped_blocks: AtomicU64::new(0),
        }
    }

    fn get_volume(&self) -> f32 {
        self.volume.load(Ordering::Relaxed) as f32 / 100.0
    }
}

impl PlaybackDevice {
    /// Create a new playback device
    pub fn new(config: AudioConfig) -> Result<Self> {
        let buffer_capacity = config.playback_buffer_samples() * 4; // Extra capacity for buffering
        let mixer = Arc::new(Mutex::new(AudioMixer::new(config.sample_rate, buffer_capacity)));
        let state = Arc::new(PlaybackState::new(config.output_volume));
        let reference_capacity = config.frame_size_samples() * config.channels as usize * 2;

        Ok(Self {
            config,
            stream: None,
            mixer,
            state,
            reference: Arc::new(Mutex::new(VecDeque::with_capacity(reference_capacity))),
        })
    }

    /// Get a reference to the mixer for adding audio
    pub fn mixer(&self) -> Arc<Mutex<AudioMixer>> {
        self.mixer.clone()
    }

    /// Recent speaker output (far-end reference for echo cancellation).
    pub fn reference_buffer(&self) -> Arc<Mutex<VecDeque<i16>>> {
        self.reference.clone()
    }

    /// List available playback devices
    pub fn list_devices() -> Result<Vec<DeviceInfo>> {
        let host = cpal::default_host();
        let default_device = host.default_output_device();
        let default_name = default_device
            .as_ref()
            .and_then(|d| d.name().ok());

        let devices = host
            .output_devices()
            .map_err(|e| AudioError::DeviceInit(format!("Failed to enumerate devices: {}", e)))?;

        let mut device_list = Vec::new();
        for device in devices {
            if let Ok(name) = device.name() {
                let is_default = default_name.as_ref().map_or(false, |dn| dn == &name);
                device_list.push(DeviceInfo { name, is_default });
            }
        }

        if device_list.is_empty() {
            device_list.push(DeviceInfo {
                name: "No devices found".to_string(),
                is_default: false,
            });
        }

        Ok(device_list)
    }

    /// Start playback
    pub fn start(&mut self) -> Result<()> {
        if self.state.is_active.load(Ordering::Relaxed) {
            return Ok(());
        }

        let host = cpal::default_host();

        // Get output device
        let device = if let Some(ref device_name) = self.config.output_device {
            host.output_devices()
                .map_err(|e| AudioError::DeviceInit(format!("Failed to enumerate devices: {}", e)))?
                .find(|d| d.name().ok().as_ref() == Some(device_name))
                .ok_or_else(|| AudioError::DeviceNotFound(device_name.clone()))?
        } else {
            host.default_output_device()
                .ok_or(AudioError::NoDevice)?
        };

        debug!("Using playback device: {:?}", device.name());

        // Negotiate a supported device format and resample when necessary.
        let (config, device_channels, device_rate, mut resampler) =
            negotiate_output_config(&device, self.config.sample_rate, self.config.channels)?;

        let mixer = self.mixer.clone();
        let state = self.state.clone();
        let config_channels = self.config.channels as usize;
        let config_rate = self.config.sample_rate;
        let reference = self.reference.clone();
        let reference_capacity =
            self.config.frame_size_samples() * self.config.channels as usize * 2;
        // Scratch buffer owned by the audio callback (allocated once).
        let mut pcm_buffer: Vec<i16> = Vec::new();

        // Build the output stream
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if !state.is_active.load(Ordering::Relaxed) {
                        // Output silence when not active
                        for sample in data.iter_mut() {
                            *sample = 0.0;
                        }
                        return;
                    }

                    let volume = state.get_volume();
                    let frames = data.len() / device_channels.max(1);
                    let needed_frames = if resampler.is_some() {
                        ((frames as f64 * config_rate as f64 / device_rate as f64).round()
                            as usize)
                            .max(1)
                    } else {
                        frames
                    };
                    let needed_samples = needed_frames * config_channels;

                    // Reuse the PCM scratch buffer across callbacks; only the
                    // first callback (or a larger device buffer) allocates.
                    if pcm_buffer.len() != needed_samples {
                        pcm_buffer.resize(needed_samples, 0);
                    }

                    {
                        // The audio callback must not block on the mixer lock:
                        // a stalled network/decoder thread would cause an
                        // underrun. Output silence for this block instead.
                        match mixer.try_lock() {
                            Ok(mut mixer_guard) => mixer_guard.mix(&mut pcm_buffer),
                            Err(_) => {
                                state.dropped_blocks.fetch_add(1, Ordering::Relaxed);
                                pcm_buffer.fill(0);
                            }
                        }
                    }

                    if device_channels == config_channels && resampler.is_none() {
                        // Fast path: no channel mapping or resampling.
                        for (i, sample) in data.iter_mut().enumerate() {
                            let pcm_sample = pcm_buffer[i] as f32 / 32768.0;
                            *sample = (pcm_sample * volume).clamp(-1.0, 1.0);
                        }
                    } else {
                        let mixed: Vec<f32> = pcm_buffer
                            .iter()
                            .map(|&sample| sample as f32 / 32768.0)
                            .collect();
                        let mapped = map_channels(&mixed, config_channels, device_channels);
                        let resampled: &[f32] = match resampler.as_mut() {
                            Some(resampler) => resampler.process(&mapped),
                            None => &mapped,
                        };
                        for (i, sample) in data.iter_mut().enumerate() {
                            let value = resampled.get(i).copied().unwrap_or(0.0);
                            *sample = (value * volume).clamp(-1.0, 1.0);
                        }
                    }

                    // Keep the most recent speaker output as AEC far-end.
                    if let Ok(mut ring) = reference.try_lock() {
                        for &sample in data.iter() {
                            if ring.len() >= reference_capacity {
                                ring.pop_front();
                            }
                            ring.push_back((sample * 32767.0) as i16);
                        }
                    }
                },
                move |err| {
                    error!("Audio playback error: {}", err);
                },
                None, // No timeout
            )
            .map_err(|e| AudioError::PlaybackOpen(format!("Failed to build output stream: {}", e)))?;

        // Start the stream
        stream
            .play()
            .map_err(|e| AudioError::PlaybackOpen(format!("Failed to start playback stream: {}", e)))?;

        self.stream = Some(stream);
        self.state.is_active.store(true, Ordering::Relaxed);

        debug!("Audio playback started");
        Ok(())
    }

    /// Stop playback
    pub fn stop(&mut self) -> Result<()> {
        self.state.is_active.store(false, Ordering::Relaxed);

        if let Some(stream) = self.stream.take() {
            drop(stream);
        }

        debug!("Audio playback stopped");
        Ok(())
    }

    /// Check if playing
    pub fn is_active(&self) -> bool {
        self.state.is_active.load(Ordering::Relaxed)
    }

    /// Snapshot of playback-path counters.
    pub fn stats(&self) -> PlaybackStats {
        let underruns = self
            .mixer
            .try_lock()
            .map(|mixer| mixer.underruns())
            .unwrap_or(0);
        PlaybackStats {
            dropped_blocks: self.state.dropped_blocks.load(Ordering::Relaxed),
            underruns,
        }
    }

    /// Set output volume (0.0-2.0)
    pub fn set_volume(&mut self, volume: f32) -> Result<()> {
        if !(0.0..=2.0).contains(&volume) {
            return Err(AudioError::InvalidConfig("Volume must be 0.0-2.0".into()));
        }
        self.state.volume.store((volume * 100.0) as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Get current volume
    pub fn volume(&self) -> f32 {
        self.state.get_volume()
    }

    /// Add audio from a specific user to the mixer
    pub fn add_user_audio(&self, user_id: u16, samples: &[i16]) {
        let mut mixer = self.mixer.lock().unwrap();
        mixer.add_audio(user_id, samples);
    }

    /// Set volume for a specific user
    pub fn set_user_volume(&self, user_id: u16, volume: f32) {
        let mut mixer = self.mixer.lock().unwrap();
        mixer.set_user_volume(user_id, volume);
    }

    /// Mute/unmute a specific user
    pub fn set_user_muted(&self, user_id: u16, muted: bool) {
        let mut mixer = self.mixer.lock().unwrap();
        mixer.set_user_muted(user_id, muted);
    }

    /// Remove a user from the mixer
    pub fn remove_user(&self, user_id: u16) {
        let mut mixer = self.mixer.lock().unwrap();
        mixer.remove_user(user_id);
    }
}

/// Audio buffer for a single user
pub struct UserAudioBuffer {
    /// User ID
    #[allow(dead_code)]
    user_id: u16,
    /// Ring buffer of samples
    buffer: Vec<i16>,
    /// Write position
    write_pos: usize,
    /// Read position
    read_pos: usize,
    /// Volume modifier for this user
    volume: f32,
    /// Is user muted
    is_muted: bool,
    /// Last activity timestamp
    last_activity: std::time::Instant,
}

impl UserAudioBuffer {
    /// Create a new buffer for a user
    pub fn new(user_id: u16, capacity: usize) -> Self {
        Self {
            user_id,
            buffer: vec![0; capacity],
            write_pos: 0,
            read_pos: 0,
            volume: 1.0,
            is_muted: false,
            last_activity: std::time::Instant::now(),
        }
    }

    /// Write samples to the buffer
    pub fn write(&mut self, samples: &[i16]) -> Result<()> {
        for &sample in samples {
            self.buffer[self.write_pos] = sample;
            self.write_pos = (self.write_pos + 1) % self.buffer.len();
        }
        self.last_activity = std::time::Instant::now();
        Ok(())
    }

    /// Read samples from the buffer
    pub fn read(&mut self, output: &mut [i16]) -> usize {
        let mut count = 0;
        for out in output.iter_mut() {
            if self.read_pos == self.write_pos {
                break;
            }
            let sample = self.buffer[self.read_pos];
            *out = if self.is_muted {
                0
            } else {
                (sample as f32 * self.volume) as i16
            };
            self.read_pos = (self.read_pos + 1) % self.buffer.len();
            count += 1;
        }
        count
    }

    /// Available samples in buffer
    pub fn available(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            self.buffer.len() - self.read_pos + self.write_pos
        }
    }

    /// Set volume for this user
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 2.0);
    }

    /// Mute/unmute this user
    pub fn set_muted(&mut self, muted: bool) {
        self.is_muted = muted;
    }

    /// Check if buffer is stale (no recent activity)
    pub fn is_stale(&self, timeout: std::time::Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }
}

/// Pick the closest supported f32 output format and a resampler when the
/// device cannot run at the requested rate.
fn negotiate_output_config(
    device: &cpal::Device,
    requested_rate: u32,
    requested_channels: u16,
) -> Result<(cpal::StreamConfig, usize, u32, Option<LinearResampler>)> {
    let ranges = device.supported_output_configs().map_err(|e| {
        AudioError::DeviceInit(format!("failed to query output formats: {e}"))
    })?;

    let mut formats = String::new();
    let mut candidates = Vec::new();
    for range in ranges {
        formats.push_str(&format!(
            " {}ch {}-{}Hz {:?};",
            range.channels(),
            range.min_sample_rate().0,
            range.max_sample_rate().0,
            range.sample_format()
        ));
        if range.sample_format() != cpal::SampleFormat::F32 {
            continue;
        }
        let rate = requested_rate.clamp(range.min_sample_rate().0, range.max_sample_rate().0);
        let channels = range.channels();
        let score = (
            u8::from(channels != requested_channels),
            rate.abs_diff(requested_rate),
            channels.abs_diff(requested_channels),
        );
        candidates.push((score, range.with_sample_rate(cpal::SampleRate(rate))));
    }

    let Some(best) = candidates
        .into_iter()
        .min_by_key(|(score, _)| *score)
        .map(|(_, config)| config)
    else {
        return Err(AudioError::DeviceInit(format!(
            "device has no f32 output format; supported:{formats}"
        )));
    };

    let device_channels = best.channels() as usize;
    let device_rate = best.sample_rate().0;
    let mut stream_config = best.config();
    stream_config.buffer_size = cpal::BufferSize::Default;
    let resampler = LinearResampler::new(requested_rate, device_rate);

    Ok((stream_config, device_channels, device_rate, resampler))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stats_start_at_zero() {
        let device = PlaybackDevice::new(AudioConfig::default()).unwrap();
        assert_eq!(device.stats(), PlaybackStats::default());
    }
}
