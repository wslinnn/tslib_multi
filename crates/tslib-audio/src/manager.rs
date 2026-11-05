//! High-level audio manager

use crate::capture::{CaptureDevice, CaptureStats, SampleBufferPool, VoiceActivityDetector};
use crate::codec::{Decoder, Encoder, OpusCodec, OpusDecoder, OpusEncoder};
use crate::config::{AudioConfig, DecoderPoolConfig};
use crate::error::{AudioError, Result};
use crate::jitter::{JitterBuffer, JitterOutput};
use crate::playback::{PlaybackDevice, PlaybackStats};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex, RwLock};

/// Per-user decoder plus the last time audio was seen from that user.
struct DecoderSlot {
    decoder: OpusDecoder,
    last_active: Instant,
}

/// Per-user fixed-delay jitter buffer and the codec of its stream.
struct JitterEntry {
    buffer: JitterBuffer,
    codec: u8,
}

/// Idle sweeps run at most this often. Decoders only hold a small Opus state,
/// so there is no point scanning the map for every 20 ms packet.
const MAX_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// High-level audio manager
pub struct AudioManager {
    config: AudioConfig,
    /// Capture device
    capture: Arc<Mutex<CaptureDevice>>,
    /// Playback device (owns the mixer)
    playback: Arc<Mutex<PlaybackDevice>>,
    /// Encoder for outgoing audio
    encoder: Arc<Mutex<OpusEncoder>>,
    /// Decoders for incoming audio (per user)
    decoders: Arc<RwLock<HashMap<u16, DecoderSlot>>>,
    /// Fixed-delay jitter buffers (one per speaking user)
    jitter: Arc<RwLock<HashMap<u16, JitterEntry>>>,
    /// Last time the decoder cache was swept for idle users
    last_sweep: StdMutex<Instant>,
    /// Successfully decoded incoming packets
    decoded_packets: AtomicU64,
    /// PLC frames generated for lost packets
    plc_frames: AtomicU64,
    /// VAD
    vad: Arc<Mutex<VoiceActivityDetector>>,
    /// Is capturing
    is_capturing: Arc<RwLock<bool>>,
}

/// Snapshot of audio pipeline counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioStats {
    /// Capture callbacks/frames counters.
    pub capture: CaptureStats,
    /// Playback blocks counter.
    pub playback: PlaybackStats,
    /// Incoming packets decoded successfully.
    pub decoded_packets: u64,
    /// PLC frames generated after packet loss.
    pub plc_frames: u64,
    /// Buffered packets across all jitter buffers.
    pub jitter_buffered: usize,
    /// Packets dropped as late by jitter buffers.
    pub jitter_late_dropped: u64,
    /// PLC frames emitted by jitter buffers for sequence gaps.
    pub jitter_plc_frames: u64,
    /// Cached per-user decoders.
    pub decoders: usize,
}

/// An encoded audio packet ready for transmission
#[derive(Debug, Clone)]
pub struct EncodedPacket {
    /// Encoded data
    pub data: Vec<u8>,
    /// Codec used
    pub codec: u8,
    /// Sequence number
    pub sequence: u16,
    /// Voice activity detected
    pub voice_activity: bool,
}

impl AudioManager {
    /// Create a new audio manager
    pub fn new(config: AudioConfig) -> Result<Self> {
        let capture = CaptureDevice::new(config.clone())?;
        let playback = PlaybackDevice::new(config.clone())?;
        let codec = OpusCodec::new(config.clone())?;
        let encoder = codec.create_encoder()?;
        let vad = VoiceActivityDetector::new(config.vad_threshold);

        Ok(Self {
            config,
            capture: Arc::new(Mutex::new(capture)),
            playback: Arc::new(Mutex::new(playback)),
            encoder: Arc::new(Mutex::new(encoder)),
            decoders: Arc::new(RwLock::new(HashMap::new())),
            jitter: Arc::new(RwLock::new(HashMap::new())),
            last_sweep: StdMutex::new(Instant::now()),
            decoded_packets: AtomicU64::new(0),
            plc_frames: AtomicU64::new(0),
            vad: Arc::new(Mutex::new(vad)),
            is_capturing: Arc::new(RwLock::new(false)),
        })
    }

    /// Start audio capture
    pub async fn start_capture(&self) -> Result<mpsc::Receiver<EncodedPacket>> {
        let (packet_tx, packet_rx) = mpsc::channel(32);
        let (frame_tx, mut frame_rx) = mpsc::channel(32);
        let sample_pool = SampleBufferPool::default();

        // Start capture device, recycling frame buffers through the pool.
        self.capture
            .lock()
            .await
            .start_with_pool(frame_tx, sample_pool.clone())?;
        *self.is_capturing.write().await = true;

        // Spawn encoding task
        let encoder = self.encoder.clone();
        let vad = self.vad.clone();
        let is_capturing = self.is_capturing.clone();
        let vad_enabled = self.config.vad_enabled;

        tokio::spawn(async move {
            let mut sequence = 0u16;
            let mut encode_buffer = vec![0u8; 1024];

            while *is_capturing.read().await {
                match frame_rx.recv().await {
                    Some(frame) => {
                        let samples = frame.samples;

                        // Check VAD
                        let voice_activity = if vad_enabled {
                            vad.lock().await.process(&samples)
                        } else {
                            true
                        };

                        // Only encode if voice detected (or VAD disabled)
                        if voice_activity {
                            let encoded = {
                                let mut enc = encoder.lock().await;
                                enc.encode(&samples, &mut encode_buffer)
                            };
                            sample_pool.recycle(samples);

                            match encoded {
                                Ok(len) => {
                                    let packet = EncodedPacket {
                                        data: encode_buffer[..len].to_vec(),
                                        codec: 4, // Opus Voice
                                        sequence,
                                        voice_activity,
                                    };
                                    sequence = sequence.wrapping_add(1);
                                    let _ = packet_tx.send(packet).await;
                                }
                                Err(e) => {
                                    tracing::warn!("Encoding error: {}", e);
                                }
                            }
                        } else {
                            sample_pool.recycle(samples);
                        }
                    }
                    None => break,
                }
            }
        });

        Ok(packet_rx)
    }

    /// Stop audio capture
    pub async fn stop_capture(&self) -> Result<()> {
        *self.is_capturing.write().await = false;
        self.capture.lock().await.stop()?;
        Ok(())
    }

    /// Start audio playback
    pub async fn start_playback(&self) -> Result<()> {
        self.playback.lock().await.start()
    }

    /// Stop audio playback
    pub async fn stop_playback(&self) -> Result<()> {
        self.playback.lock().await.stop()
    }

    /// Process incoming audio from a user
    pub async fn process_incoming(&self, user_id: u16, data: &[u8], codec: u8) -> Result<()> {
        ensure_opus_codec(codec)?;

        let mut samples =
            vec![0i16; self.config.frame_size_samples() * self.config.channels as usize];
        let now = Instant::now();

        let decoded = {
            let mut decoders = self.decoders.write().await;
            self.sweep_if_due(&mut decoders, now);

            if !decoders.contains_key(&user_id) {
                let decoder = OpusDecoder::new(&self.config)?;
                decoders.insert(
                    user_id,
                    DecoderSlot {
                        decoder,
                        last_active: now,
                    },
                );
                enforce_decoder_limit(
                    &mut decoders,
                    self.config.decoder_pool.max_decoders,
                    Some(user_id),
                );
            }

            let slot = decoders
                .get_mut(&user_id)
                .expect("decoder was just inserted");
            slot.last_active = now;
            let decoded = slot.decoder.decode(data, &mut samples)?;
            self.decoded_packets.fetch_add(1, Ordering::Relaxed);
            decoded
        };

        // Add to playback mixer
        let playback = self.playback.lock().await;
        playback.add_user_audio(user_id, &samples[..decoded]);

        Ok(())
    }

    /// Handle packet loss for a user
    pub async fn handle_packet_loss(&self, user_id: u16) -> Result<()> {
        let now = Instant::now();
        let (samples, decoded) = {
            let mut decoders = self.decoders.write().await;
            let Some(slot) = decoders.get_mut(&user_id) else {
                return Ok(());
            };
            slot.last_active = now;
            let mut samples =
                vec![0i16; self.config.frame_size_samples() * self.config.channels as usize];
            let decoded = slot.decoder.decode_plc(&mut samples)?;
            self.plc_frames.fetch_add(1, Ordering::Relaxed);
            (samples, decoded)
        };

        let playback = self.playback.lock().await;
        playback.add_user_audio(user_id, &samples[..decoded]);
        Ok(())
    }

    /// Process a voice packet through the fixed-delay jitter buffer.
    ///
    /// Packets are held until `jitter_buffer.target_delay_ms` has elapsed
    /// since the first packet of the burst, then played in sequence order.
    /// Missing sequence numbers trigger up to `max_plc_frames` PLC frames.
    /// Call [`Self::pump_jitter`] periodically so buffered packets are
    /// released when no new packets arrive.
    pub async fn process_incoming_with_seq(
        &self,
        user_id: u16,
        seq: u16,
        data: &[u8],
        codec: u8,
    ) -> Result<()> {
        ensure_opus_codec(codec)?;
        let now = Instant::now();

        let outputs = {
            let mut jitter = self.jitter.write().await;
            let entry = jitter.entry(user_id).or_insert_with(|| JitterEntry {
                buffer: JitterBuffer::new(self.config.jitter_buffer.clone()),
                codec,
            });
            entry.codec = codec;
            entry.buffer.push(seq, data.to_vec(), now);
            entry.buffer.pump(now)
        };

        self.play_jitter_outputs(user_id, codec, outputs).await
    }

    /// Release jitter-buffered packets whose play deadline has passed.
    ///
    /// Embeddings with a sequence-aware receive path should call this on a
    /// short timer (for example every 10–20 ms). Without it, packets buffered
    /// after the last arrival remain unplayed until the next packet arrives.
    pub async fn pump_jitter(&self) -> Result<()> {
        let now = Instant::now();
        let pending: Vec<(u16, u8, Vec<JitterOutput>)> = {
            let mut jitter = self.jitter.write().await;
            jitter
                .iter_mut()
                .filter_map(|(&user_id, entry)| {
                    let outputs = entry.buffer.pump(now);
                    (!outputs.is_empty()).then_some((user_id, entry.codec, outputs))
                })
                .collect()
        };

        for (user_id, codec, outputs) in pending {
            self.play_jitter_outputs(user_id, codec, outputs).await?;
        }
        Ok(())
    }

    async fn play_jitter_outputs(
        &self,
        user_id: u16,
        codec: u8,
        outputs: Vec<JitterOutput>,
    ) -> Result<()> {
        for output in outputs {
            match output {
                JitterOutput::Packet(data) => {
                    self.process_incoming(user_id, &data, codec).await?;
                }
                JitterOutput::Plc => {
                    self.handle_packet_loss(user_id).await?;
                }
            }
        }
        Ok(())
    }

    /// Set input volume
    pub async fn set_input_volume(&self, volume: f32) -> Result<()> {
        self.capture.lock().await.set_volume(volume)
    }

    /// Set output volume
    pub async fn set_output_volume(&self, volume: f32) -> Result<()> {
        self.playback.lock().await.set_volume(volume)
    }

    /// Set volume for a specific user
    pub async fn set_user_volume(&self, user_id: u16, volume: f32) {
        let playback = self.playback.lock().await;
        playback.set_user_volume(user_id, volume);
    }

    /// Mute/unmute a specific user
    pub async fn set_user_muted(&self, user_id: u16, muted: bool) {
        let playback = self.playback.lock().await;
        playback.set_user_muted(user_id, muted);
    }

    /// Remove a user's audio state
    pub async fn remove_user(&self, user_id: u16) {
        self.decoders.write().await.remove(&user_id);
        self.jitter.write().await.remove(&user_id);
        let playback = self.playback.lock().await;
        playback.remove_user(user_id);
    }

    /// Release per-user audio state in response to a core event.
    ///
    /// Call this for every event returned by the client. `UserLeft` drops the
    /// decoder and mixer buffer for that user, so a long session does not keep
    /// state for users that are gone.
    pub async fn handle_event(&self, event: &tslib_core::Event) {
        if let tslib_core::Event::UserLeft { user, .. } = event {
            self.remove_user(user.id).await;
        }
    }

    /// Apply a batch of core events; see [`Self::handle_event`].
    pub async fn handle_events<'a, I>(&self, events: I)
    where
        I: IntoIterator<Item = &'a tslib_core::Event>,
    {
        for event in events {
            self.handle_event(event).await;
        }
    }

    /// Remove decoders idle for longer than `decoder_pool.idle_timeout`.
    ///
    /// `process_incoming` sweeps automatically; call this after quiet periods
    /// when no packets arrive to reclaim memory sooner. Returns the number of
    /// decoders removed.
    pub async fn evict_idle_decoders(&self) -> usize {
        let now = Instant::now();
        let (removed, remaining) = {
            let mut decoders = self.decoders.write().await;
            let removed = self.evict_idle_locked(&mut decoders, now);
            let remaining: Vec<u16> = decoders.keys().copied().collect();
            (removed, remaining)
        };

        if removed > 0 {
            // Drop jitter buffers whose decoder was evicted; otherwise they
            // would keep buffered packets for users that are no longer active.
            let mut jitter = self.jitter.write().await;
            jitter.retain(|user_id, _| remaining.contains(user_id));
        }
        removed
    }

    /// Number of cached per-user decoders (diagnostics).
    pub async fn decoder_count(&self) -> usize {
        self.decoders.read().await.len()
    }

    /// Snapshot of capture, playback and decoder counters.
    ///
    /// Counters are cumulative since the manager was created. They are
    /// intended for diagnostics dashboards, not per-packet control flow.
    pub async fn stats(&self) -> AudioStats {
        let capture = self.capture.lock().await.stats();
        let playback = self.playback.lock().await.stats();
        let decoders = self.decoders.read().await.len();
        let (jitter_buffered, jitter_late_dropped, jitter_plc_frames) = {
            let jitter = self.jitter.read().await;
            let mut buffered = 0usize;
            let mut late = 0u64;
            let mut plc = 0u64;
            for entry in jitter.values() {
                let stats = entry.buffer.stats();
                buffered += stats.buffered;
                late += stats.late_dropped;
                plc += stats.plc_frames;
            }
            (buffered, late, plc)
        };

        AudioStats {
            capture,
            playback,
            decoded_packets: self.decoded_packets.load(Ordering::Relaxed),
            plc_frames: self.plc_frames.load(Ordering::Relaxed),
            jitter_buffered,
            jitter_late_dropped,
            jitter_plc_frames,
            decoders,
        }
    }

    /// Sweep idle decoders if the sweep interval has elapsed.
    fn sweep_if_due(&self, decoders: &mut HashMap<u16, DecoderSlot>, now: Instant) {
        let interval = sweep_interval(&self.config.decoder_pool);
        let due = {
            let mut last = self
                .last_sweep
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if now.saturating_duration_since(*last) >= interval {
                *last = now;
                true
            } else {
                false
            }
        };
        if due {
            self.evict_idle_locked(decoders, now);
        }
    }

    fn evict_idle_locked(&self, decoders: &mut HashMap<u16, DecoderSlot>, now: Instant) -> usize {
        let Some(idle_timeout) = self.config.decoder_pool.idle_timeout else {
            return 0;
        };
        let before = decoders.len();
        decoders.retain(|_, slot| now.saturating_duration_since(slot.last_active) < idle_timeout);
        before - decoders.len()
    }

    /// Get VAD state
    pub async fn is_voice_active(&self) -> bool {
        self.vad.lock().await.energy() > self.config.vad_threshold
    }

    /// Get current voice energy level
    pub async fn voice_energy(&self) -> f32 {
        self.vad.lock().await.energy()
    }
}

/// Interval between automatic idle sweeps. A short idle timeout is swept at
/// the same scale so tests and aggressive configurations take effect promptly.
fn sweep_interval(pool: &DecoderPoolConfig) -> Duration {
    match pool.idle_timeout {
        Some(timeout) if timeout < MAX_SWEEP_INTERVAL => timeout.max(Duration::from_millis(100)),
        _ => MAX_SWEEP_INTERVAL,
    }
}

/// Evict least-recently-used decoders until the cache fits `max` entries.
///
/// `keep` is the user whose packet is currently being handled and is never
/// evicted. Returns the number of decoders removed.
fn enforce_decoder_limit(
    decoders: &mut HashMap<u16, DecoderSlot>,
    max: usize,
    keep: Option<u16>,
) -> usize {
    let max = max.max(1);
    let mut removed = 0;
    while decoders.len() > max {
        let oldest = decoders
            .iter()
            .filter(|&(id, _)| Some(*id) != keep)
            .min_by_key(|&(_, slot)| slot.last_active)
            .map(|(id, _)| *id);
        match oldest {
            Some(id) => {
                decoders.remove(&id);
                removed += 1;
            }
            None => break,
        }
    }
    removed
}

/// Only Opus voice/music are decoded by `AudioManager`; other codecs must not
/// be fed to the Opus decoder.
fn ensure_opus_codec(codec: u8) -> Result<()> {
    match tslib_core::AudioCodec::from_id(codec) {
        Some(codec) if codec.is_opus() => Ok(()),
        _ => Err(AudioError::UnsupportedCodec(codec)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn insert_decoders(audio: &AudioManager, entries: &[(u16, Instant)]) {
        let mut decoders = audio.decoders.write().await;
        for &(user_id, last_active) in entries {
            decoders.insert(
                user_id,
                DecoderSlot {
                    decoder: OpusDecoder::new(&audio.config).unwrap(),
                    last_active,
                },
            );
        }
    }

    #[test]
    fn accepts_opus_codecs() {
        assert!(ensure_opus_codec(4).is_ok());
        assert!(ensure_opus_codec(5).is_ok());
    }

    #[test]
    fn rejects_non_opus_codecs() {
        for codec in [0, 1, 2, 3, 6, 255] {
            assert!(matches!(
                ensure_opus_codec(codec),
                Err(AudioError::UnsupportedCodec(value)) if value == codec
            ));
        }
    }

    #[tokio::test]
    async fn jitter_delays_then_decodes() {
        let config = AudioConfig::default();
        let audio = AudioManager::new(config.clone()).unwrap();
        let codec = OpusCodec::new(config.clone()).unwrap();
        let mut encoder = codec.create_encoder().unwrap();

        let pcm: Vec<i16> = (0..config.frame_size_samples() as i16).collect();
        let mut encoded = vec![0u8; 4000];
        let len = encoder.encode(&pcm, &mut encoded).unwrap();

        audio
            .process_incoming_with_seq(7, 1, &encoded[..len], 4)
            .await
            .unwrap();

        let stats = audio.stats().await;
        assert_eq!(stats.jitter_buffered, 1);
        assert_eq!(stats.decoded_packets, 0);

        tokio::time::sleep(Duration::from_millis(70)).await;
        audio.pump_jitter().await.unwrap();

        let stats = audio.stats().await;
        assert_eq!(stats.decoded_packets, 1);
        assert_eq!(stats.jitter_buffered, 0);
    }

    #[tokio::test]
    async fn stats_start_at_zero() {
        let audio = AudioManager::new(AudioConfig::default()).unwrap();
        assert_eq!(audio.stats().await, AudioStats::default());
    }

    #[tokio::test]
    async fn plc_updates_counter() {
        let audio = AudioManager::new(AudioConfig::default()).unwrap();
        insert_decoders(&audio, &[(7, Instant::now())]).await;

        audio.handle_packet_loss(7).await.unwrap();
        assert_eq!(audio.stats().await.plc_frames, 1);
    }

    #[tokio::test]
    async fn user_left_event_removes_decoder() {
        let audio = AudioManager::new(AudioConfig::default()).unwrap();
        let now = Instant::now();
        insert_decoders(&audio, &[(3, now), (7, now)]).await;

        let event = tslib_core::Event::UserLeft {
            user: tslib_core::User {
                id: 7,
                ..Default::default()
            },
            reason: "left".to_string(),
        };
        audio.handle_event(&event).await;

        assert_eq!(audio.decoder_count().await, 1);
        assert!(audio.decoders.read().await.contains_key(&3));
    }

    #[tokio::test]
    async fn idle_decoders_are_evicted() {
        let audio = AudioManager::new(AudioConfig::default()).unwrap();
        let now = Instant::now();
        insert_decoders(&audio, &[(1, now - Duration::from_secs(600)), (2, now)]).await;

        assert_eq!(audio.evict_idle_decoders().await, 1);
        assert_eq!(audio.decoder_count().await, 1);
        assert!(audio.decoders.read().await.contains_key(&2));
    }

    #[tokio::test]
    async fn idle_timeout_none_keeps_decoders() {
        let mut config = AudioConfig::default();
        config.decoder_pool.idle_timeout = None;
        let audio = AudioManager::new(config).unwrap();
        let now = Instant::now();
        insert_decoders(&audio, &[(1, now - Duration::from_secs(3600))]).await;

        assert_eq!(audio.evict_idle_decoders().await, 0);
        assert_eq!(audio.decoder_count().await, 1);
    }

    #[tokio::test]
    async fn automatic_sweep_runs_when_due() {
        let mut config = AudioConfig::default();
        config.decoder_pool.idle_timeout = Some(Duration::from_millis(100));
        let audio = AudioManager::new(config).unwrap();
        let now = Instant::now();
        insert_decoders(&audio, &[(1, now - Duration::from_secs(1)), (2, now)]).await;
        // Pretend the last sweep happened long enough ago to be due again.
        *audio.last_sweep.lock().unwrap() = now - Duration::from_secs(1);

        {
            let mut decoders = audio.decoders.write().await;
            audio.sweep_if_due(&mut decoders, Instant::now());
        }

        assert_eq!(audio.decoder_count().await, 1);
        assert!(audio.decoders.read().await.contains_key(&2));
    }

    #[tokio::test]
    async fn capacity_evicts_least_recently_used() {
        let mut config = AudioConfig::default();
        config.decoder_pool.max_decoders = 2;
        let audio = AudioManager::new(config).unwrap();
        let now = Instant::now();
        insert_decoders(
            &audio,
            &[
                (1, now - Duration::from_secs(30)),
                (2, now - Duration::from_secs(20)),
                (3, now),
            ],
        )
        .await;

        let removed = {
            let mut decoders = audio.decoders.write().await;
            enforce_decoder_limit(&mut decoders, 2, Some(3))
        };

        assert_eq!(removed, 1);
        let decoders = audio.decoders.read().await;
        assert!(!decoders.contains_key(&1));
        assert!(decoders.contains_key(&2));
        assert!(decoders.contains_key(&3));
    }

    #[tokio::test]
    async fn zero_capacity_still_keeps_current_user() {
        let audio = AudioManager::new(AudioConfig::default()).unwrap();
        let now = Instant::now();
        insert_decoders(&audio, &[(1, now), (2, now), (3, now)]).await;

        let removed = {
            let mut decoders = audio.decoders.write().await;
            enforce_decoder_limit(&mut decoders, 0, Some(2))
        };

        assert_eq!(removed, 2);
        assert_eq!(audio.decoder_count().await, 1);
        assert!(audio.decoders.read().await.contains_key(&2));
    }
}
