//! Audio capture (microphone input)

use crate::config::AudioConfig;
use crate::error::{AudioError, Result};
use crate::resample::{map_channels, LinearResampler};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, error};

/// Audio capture device
pub struct CaptureDevice {
    config: AudioConfig,
    /// The cpal stream handle
    stream: Option<cpal::Stream>,
    /// Shared state for the capture thread
    state: Arc<CaptureState>,
    /// Start time for timestamps
    start_time: Option<Instant>,
}

/// Counters for the realtime capture path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureStats {
    /// Frames cut from the accumulation buffer.
    pub frames_captured: u64,
    /// Frames dropped because the encoder channel was full.
    pub frames_dropped: u64,
    /// Input callbacks dropped because the buffer lock was contended.
    pub callbacks_dropped: u64,
}

/// Shared state between capture thread and main thread
struct CaptureState {
    /// Is capturing active
    is_active: AtomicBool,
    /// Input volume (scaled by 100 for atomic ops)
    volume: AtomicU64,
    /// VAD threshold (scaled by 1000 for atomic ops)
    vad_threshold: AtomicU64,
    /// VAD enabled
    vad_enabled: AtomicBool,
    /// Automatic gain control enabled
    agc_enabled: AtomicBool,
    /// AGC target level (scaled by 1000)
    agc_target: AtomicU64,
    /// Noise suppression enabled
    noise_suppression: AtomicBool,
    /// Captured frame counter
    frames_captured: AtomicU64,
    /// Dropped frame counter
    frames_dropped: AtomicU64,
    /// Dropped callback counter
    callbacks_dropped: AtomicU64,
}

impl CaptureState {
    fn new(config: &AudioConfig) -> Self {
        Self {
            is_active: AtomicBool::new(false),
            volume: AtomicU64::new((config.input_volume * 100.0) as u64),
            vad_threshold: AtomicU64::new((config.vad_threshold * 1000.0) as u64),
            vad_enabled: AtomicBool::new(config.vad_enabled),
            agc_enabled: AtomicBool::new(config.agc_enabled),
            agc_target: AtomicU64::new((config.agc_target.abs() * 1000.0) as u64),
            noise_suppression: AtomicBool::new(config.noise_suppression),
            frames_captured: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            callbacks_dropped: AtomicU64::new(0),
        }
    }

    fn get_volume(&self) -> f32 {
        self.volume.load(Ordering::Relaxed) as f32 / 100.0
    }

    fn get_vad_threshold(&self) -> f32 {
        self.vad_threshold.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

/// A captured audio frame
#[derive(Debug, Clone)]
pub struct AudioFrame {
    /// PCM samples (16-bit signed)
    pub samples: Vec<i16>,
    /// Timestamp in microseconds
    pub timestamp_us: u64,
    /// Voice activity detected
    pub voice_activity: bool,
}

/// Reusable PCM frame buffers shared between the realtime capture callback
/// and the encoder task.
///
/// The callback takes a buffer from the pool for each frame and the consumer
/// returns it with [`SampleBufferPool::recycle`] after encoding, so
/// steady-state capture does not allocate on the audio thread.
#[derive(Clone)]
pub struct SampleBufferPool {
    buffers: Arc<Mutex<Vec<Vec<i16>>>>,
    max_buffers: usize,
}

impl SampleBufferPool {
    /// Create a pool that keeps at most `max_buffers` recycled buffers.
    pub fn new(max_buffers: usize) -> Self {
        Self {
            buffers: Arc::new(Mutex::new(Vec::new())),
            max_buffers: max_buffers.max(1),
        }
    }

    /// Return an encoded frame buffer to the pool for reuse.
    pub fn recycle(&self, buffer: Vec<i16>) {
        if buffer.capacity() == 0 {
            return;
        }
        let mut buffers = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        if buffers.len() < self.max_buffers {
            buffers.push(buffer);
        }
    }

    fn acquire(&self, len: usize) -> Vec<i16> {
        let mut buffer = self
            .buffers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop()
            .unwrap_or_default();
        buffer.clear();
        buffer.reserve(len);
        buffer
    }
}

impl Default for SampleBufferPool {
    fn default() -> Self {
        Self::new(8)
    }
}

impl CaptureDevice {
    /// Create a new capture device
    pub fn new(config: AudioConfig) -> Result<Self> {
        let state = Arc::new(CaptureState::new(&config));
        Ok(Self {
            config,
            stream: None,
            state,
            start_time: None,
        })
    }

    /// List available capture devices
    pub fn list_devices() -> Result<Vec<DeviceInfo>> {        let host = cpal::default_host();
        let default_device = host.default_input_device();
        let default_name = default_device
            .as_ref()
            .and_then(|d| d.name().ok());

        let devices = host
            .input_devices()
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

    /// Start capturing audio
    pub fn start(&mut self, frame_tx: mpsc::Sender<AudioFrame>) -> Result<()> {
        self.start_with_pool(frame_tx, SampleBufferPool::default())
    }

    /// Start capturing audio, reusing frame buffers from `pool`.
    ///
    /// The consumer is expected to call [`SampleBufferPool::recycle`] with
    /// `AudioFrame::samples` once it no longer needs the frame. Without
    /// recycling the capture thread falls back to one allocation per frame.
    pub fn start_with_pool(
        &mut self,
        frame_tx: mpsc::Sender<AudioFrame>,
        pool: SampleBufferPool,
    ) -> Result<()> {
        if self.state.is_active.load(Ordering::Relaxed) {
            return Ok(());
        }

        let host = cpal::default_host();

        // Get input device
        let device = if let Some(ref device_name) = self.config.input_device {
            host.input_devices()
                .map_err(|e| AudioError::DeviceInit(format!("Failed to enumerate devices: {}", e)))?
                .find(|d| d.name().ok().as_ref() == Some(device_name))
                .ok_or_else(|| AudioError::DeviceNotFound(device_name.clone()))?
        } else {
            host.default_input_device()
                .ok_or(AudioError::NoDevice)?
        };

        debug!("Using capture device: {:?}", device.name());

        // Negotiate a supported device format and enable resampling when the
        // device cannot run at the requested 48 kHz (FR-AU-05/08).
        let (config, device_channels, resampler) =
            negotiate_input_config(&device, self.config.sample_rate, self.config.channels)?;

        let frame_size = self.config.frame_size_samples();
        let channels = self.config.channels as usize;
        let state = self.state.clone();
        let start_time = Instant::now();
        self.start_time = Some(start_time);
        let mut resampler = resampler;
        let mut high_pass_x = 0.0f32;
        let mut high_pass_y = 0.0f32;
        let mut agc_gain = 1.0f32;

        // Buffer to accumulate samples
        let buffer: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::with_capacity(frame_size * channels * 2)));
        let buffer_clone = buffer.clone();
        let pool_clone = pool.clone();

        // Build the input stream
        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if !state.is_active.load(Ordering::Relaxed) {
                        return;
                    }

                    let volume = state.get_volume();
                    let vad_threshold = state.get_vad_threshold();
                    let vad_enabled = state.vad_enabled.load(Ordering::Relaxed);
                    let agc_enabled = state.agc_enabled.load(Ordering::Relaxed);
                    let agc_target = state.agc_target.load(Ordering::Relaxed) as f32 / 1000.0;
                    let noise_suppression = state.noise_suppression.load(Ordering::Relaxed);

                    // Convert f32 to i16 and apply volume directly into the
                    // accumulation buffer. Never block the realtime capture
                    // thread on this lock; a contended callback is dropped
                    // rather than stalling the device.
                    let Ok(mut buf) = buffer_clone.try_lock() else {
                        state.callbacks_dropped.fetch_add(1, Ordering::Relaxed);
                        return;
                    };
                    let convert = |sample: f32| -> i16 {
                        (sample.clamp(-1.0, 1.0) * 32767.0) as i16
                    };

                    if device_channels == channels && resampler.is_none() {
                        // Fast path: no channel mapping or resampling.
                        buf.reserve(data.len());
                        buf.extend(data.iter().map(|&sample| convert(sample * volume)));
                    } else {
                        let mapped = if device_channels == channels {
                            data.to_vec()
                        } else {
                            map_channels(data, device_channels, channels)
                        };
                        let scaled: Vec<f32> =
                            mapped.iter().map(|&sample| sample * volume).collect();
                        let resampled: &[f32] = match resampler.as_mut() {
                            Some(resampler) => resampler.process(&scaled),
                            None => &scaled,
                        };
                        buf.extend(resampled.iter().map(|&sample| convert(sample)));
                    }

                    // Check if we have enough for a frame
                    let samples_per_frame = frame_size * channels;
                    let mut consumed = 0;
                    while buf.len() - consumed >= samples_per_frame {
                        let mut frame_samples = pool_clone.acquire(samples_per_frame);
                        frame_samples
                            .extend_from_slice(&buf[consumed..consumed + samples_per_frame]);
                        consumed += samples_per_frame;

                        // Noise suppression (high-pass) and AGC run before VAD
                        // and encoding, so both see the processed signal.
                        if noise_suppression {
                            high_pass(
                                &mut frame_samples,
                                &mut high_pass_x,
                                &mut high_pass_y,
                            );
                        }
                        if agc_enabled {
                            apply_agc(&mut frame_samples, agc_target, &mut agc_gain);
                        }

                        // Calculate voice activity
                        let voice_activity = if vad_enabled {
                            calculate_vad(&frame_samples, vad_threshold)
                        } else {
                            true
                        };

                        let timestamp_us = start_time.elapsed().as_micros() as u64;

                        let frame = AudioFrame {
                            samples: frame_samples,
                            timestamp_us,
                            voice_activity,
                        };
                        state.frames_captured.fetch_add(1, Ordering::Relaxed);

                        // Try to send, but don't block if receiver is full
                        if let Err(e) = frame_tx.try_send(frame) {
                            match e {
                                mpsc::error::TrySendError::Full(frame) => {
                                    // Channel full, drop frame (acceptable for
                                    // real-time audio) and keep its buffer.
                                    state.frames_dropped.fetch_add(1, Ordering::Relaxed);
                                    pool_clone.recycle(frame.samples);
                                }
                                mpsc::error::TrySendError::Closed(_) => {
                                    // Channel closed, stop capturing
                                    state.is_active.store(false, Ordering::Relaxed);
                                    return;
                                }
                            }
                        }
                    }

                    if consumed > 0 {
                        let remaining = buf.len() - consumed;
                        buf.copy_within(consumed.., 0);
                        buf.truncate(remaining);
                    }
                },
                move |err| {
                    error!("Audio capture error: {}", err);
                },
                None, // No timeout
            )
            .map_err(|e| AudioError::CaptureOpen(format!("Failed to build input stream: {}", e)))?;

        // Start the stream
        stream
            .play()
            .map_err(|e| AudioError::CaptureOpen(format!("Failed to start capture stream: {}", e)))?;

        self.stream = Some(stream);
        self.state.is_active.store(true, Ordering::Relaxed);

        debug!("Audio capture started");
        Ok(())
    }

    /// Stop capturing audio
    pub fn stop(&mut self) -> Result<()> {
        self.state.is_active.store(false, Ordering::Relaxed);

        if let Some(stream) = self.stream.take() {
            // Stream will be dropped and stopped
            drop(stream);
        }

        self.start_time = None;
        debug!("Audio capture stopped");
        Ok(())
    }

    /// Check if capturing
    pub fn is_active(&self) -> bool {
        self.state.is_active.load(Ordering::Relaxed)
    }

    /// Snapshot of capture-path counters.
    pub fn stats(&self) -> CaptureStats {
        CaptureStats {
            frames_captured: self.state.frames_captured.load(Ordering::Relaxed),
            frames_dropped: self.state.frames_dropped.load(Ordering::Relaxed),
            callbacks_dropped: self.state.callbacks_dropped.load(Ordering::Relaxed),
        }
    }

    /// Set input volume (0.0-2.0)
    pub fn set_volume(&mut self, volume: f32) -> Result<()> {
        if !(0.0..=2.0).contains(&volume) {
            return Err(AudioError::InvalidConfig("Volume must be 0.0-2.0".into()));
        }
        self.state.volume.store((volume * 100.0) as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Set VAD threshold (0.0-1.0)
    pub fn set_vad_threshold(&mut self, threshold: f32) -> Result<()> {
        if !(0.0..=1.0).contains(&threshold) {
            return Err(AudioError::InvalidConfig("VAD threshold must be 0.0-1.0".into()));
        }
        self.state.vad_threshold.store((threshold * 1000.0) as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Enable/disable VAD
    pub fn set_vad_enabled(&mut self, enabled: bool) {
        self.state.vad_enabled.store(enabled, Ordering::Relaxed);
    }
}

/// Pick the closest supported f32 input format and a resampler when the
/// device cannot run at the requested rate.
fn negotiate_input_config(
    device: &cpal::Device,
    requested_rate: u32,
    requested_channels: u16,
) -> Result<(cpal::StreamConfig, usize, Option<LinearResampler>)> {
    let ranges = device.supported_input_configs().map_err(|e| {
        AudioError::DeviceInit(format!("failed to query input formats: {e}"))
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
            "device has no f32 input format; supported:{formats}"
        )));
    };

    let device_channels = best.channels() as usize;
    let device_rate = best.sample_rate().0;
    let mut stream_config = best.config();
    stream_config.buffer_size = cpal::BufferSize::Default;
    let resampler = LinearResampler::new(device_rate, requested_rate);

    Ok((stream_config, device_channels, resampler))
}

/// First-order high-pass (~80 Hz at 48 kHz) used as lightweight noise
/// suppression: removes DC offset and low-frequency rumble without gating
/// speech.
fn high_pass(samples: &mut [i16], previous_input: &mut f32, previous_output: &mut f32) {
    const ALPHA: f32 = 0.995;
    for sample in samples {
        let input = *sample as f32;
        let output = ALPHA * (*previous_output + input - *previous_input);
        *previous_input = input;
        *previous_output = output;
        *sample = output.clamp(-32768.0, 32767.0) as i16;
    }
}

/// Gentle automatic gain control: move the frame RMS toward `target`
/// (linear full-scale, e.g. 0.125 for -18 dBFS) with a slow, bounded gain.
fn apply_agc(samples: &mut [i16], target: f32, gain: &mut f32) {
    if samples.is_empty() || target <= 0.0 {
        return;
    }
    let sum: f64 = samples
        .iter()
        .map(|&sample| (sample as f64).powi(2))
        .sum();
    let rms = (sum / samples.len() as f64).sqrt() as f32 / 32768.0;
    if rms < 1e-5 {
        return;
    }

    let desired = (target / rms).clamp(0.5, 2.0);
    *gain = 0.99 * *gain + 0.01 * desired;
    for sample in samples {
        *sample = ((*sample as f32) * *gain).clamp(-32768.0, 32767.0) as i16;
    }
}

/// Calculate voice activity detection based on RMS energy
fn calculate_vad(samples: &[i16], threshold: f32) -> bool {    if samples.is_empty() {
        return false;
    }

    let sum: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
    let rms = (sum / samples.len() as f64).sqrt() as f32;
    let energy = rms / 32768.0;

    energy > threshold
}

/// Device information
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Device name
    pub name: String,
    /// Is this the system default device
    pub is_default: bool,
}

/// Voice Activity Detector
pub struct VoiceActivityDetector {
    threshold: f32,
    /// Smoothing factor
    smoothing: f32,
    /// Current energy level
    current_energy: f32,
}

impl VoiceActivityDetector {
    /// Create a new VAD
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold,
            smoothing: 0.95,
            current_energy: 0.0,
        }
    }

    /// Process samples and detect voice activity
    pub fn process(&mut self, samples: &[i16]) -> bool {
        // Calculate RMS energy
        let sum: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
        let rms = (sum / samples.len() as f64).sqrt() as f32;

        // Normalize to 0-1 range
        let energy = rms / 32768.0;

        // Smooth the energy
        self.current_energy = self.smoothing * self.current_energy + (1.0 - self.smoothing) * energy;

        self.current_energy > self.threshold
    }

    /// Set the threshold
    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold.clamp(0.0, 1.0);
    }

    /// Get current energy level
    pub fn energy(&self) -> f32 {
        self.current_energy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_start_at_zero() {
        let device = CaptureDevice::new(AudioConfig::default()).unwrap();
        assert_eq!(device.stats(), CaptureStats::default());
    }

    #[test]
    fn sample_pool_reuses_buffers() {        let pool = SampleBufferPool::new(2);
        let buffer = pool.acquire(8);
        assert!(buffer.capacity() >= 8);
        pool.recycle(buffer);

        let reused = pool.acquire(8);
        assert!(reused.capacity() >= 8);

        pool.recycle(vec![0i16; 8]);
        pool.recycle(vec![0i16; 8]);
        pool.recycle(vec![0i16; 8]);
        assert_eq!(pool.buffers.lock().unwrap().len(), 2);
    }

    #[test]
    fn sample_pool_ignores_empty_buffers() {
        let pool = SampleBufferPool::new(4);
        pool.recycle(Vec::new());
        assert!(pool.buffers.lock().unwrap().is_empty());
    }

    #[test]
    fn vad_detects_energy() {
        assert!(!calculate_vad(&[0i16; 960], 0.3));
        assert!(calculate_vad(&[10_000i16; 960], 0.3));
    }

    #[test]
    fn high_pass_removes_dc() {
        let mut samples = [1000i16; 960];
        let mut previous_input = 0.0;
        let mut previous_output = 0.0;
        high_pass(&mut samples, &mut previous_input, &mut previous_output);
        assert!(samples[959].abs() < 100);
    }

    #[test]
    fn agc_moves_quiet_frames_toward_target() {
        let mut gain = 1.0f32;
        let mut samples = vec![500i16; 960];
        for _ in 0..200 {
            samples.fill(500);
            apply_agc(&mut samples, 0.125, &mut gain);
        }
        assert!(gain > 1.5);
        assert!(samples[0] > 500);
    }
}
