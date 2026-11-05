//! Audio mixer for combining multiple audio streams

use crate::playback::UserAudioBuffer;
use std::collections::HashMap;

/// Audio mixer that combines multiple user streams
pub struct AudioMixer {
    /// Per-user audio buffers
    user_buffers: HashMap<u16, UserAudioBuffer>,
    /// Master volume
    master_volume: f32,
    /// Buffer capacity per user
    buffer_capacity: usize,
    /// Sample rate
    #[allow(dead_code)]
    sample_rate: u32,
    /// Reused per-user read scratch space (avoids allocating per callback)
    temp: Vec<i16>,
}

impl AudioMixer {
    /// Create a new mixer
    pub fn new(sample_rate: u32, buffer_capacity: usize) -> Self {
        Self {
            user_buffers: HashMap::new(),
            master_volume: 1.0,
            buffer_capacity,
            sample_rate,
            temp: Vec::new(),
        }
    }

    /// Add audio from a user
    pub fn add_audio(&mut self, user_id: u16, samples: &[i16]) {
        let buffer = self
            .user_buffers
            .entry(user_id)
            .or_insert_with(|| UserAudioBuffer::new(user_id, self.buffer_capacity));

        let _ = buffer.write(samples);
    }

    /// Mix all audio streams into output buffer
    pub fn mix(&mut self, output: &mut [i16]) {
        // Clear output
        for sample in output.iter_mut() {
            *sample = 0;
        }

        // Temporary buffer for each user's contribution, reused across
        // callbacks so mixing does not allocate on the audio thread.
        if self.temp.len() != output.len() {
            self.temp.resize(output.len(), 0);
        }

        // Mix all active users
        for buffer in self.user_buffers.values_mut() {
            let samples_read = buffer.read(&mut self.temp);

            // Add to output with saturation
            for i in 0..samples_read {
                let mixed = output[i] as i32 + self.temp[i] as i32;
                output[i] = mixed.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
        }

        // Apply master volume
        if (self.master_volume - 1.0).abs() > f32::EPSILON {
            for sample in output.iter_mut() {
                *sample = (*sample as f32 * self.master_volume) as i16;
            }
        }

        // Cleanup stale buffers
        self.cleanup_stale();
    }

    /// Set volume for a specific user
    pub fn set_user_volume(&mut self, user_id: u16, volume: f32) {
        if let Some(buffer) = self.user_buffers.get_mut(&user_id) {
            buffer.set_volume(volume);
        }
    }

    /// Mute/unmute a specific user
    pub fn set_user_muted(&mut self, user_id: u16, muted: bool) {
        if let Some(buffer) = self.user_buffers.get_mut(&user_id) {
            buffer.set_muted(muted);
        }
    }

    /// Set master volume
    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = volume.clamp(0.0, 2.0);
    }

    /// Get master volume
    pub fn master_volume(&self) -> f32 {
        self.master_volume
    }

    /// Remove a user's buffer
    pub fn remove_user(&mut self, user_id: u16) {
        self.user_buffers.remove(&user_id);
    }

    /// Clear all buffers
    pub fn clear(&mut self) {
        self.user_buffers.clear();
    }

    /// Cleanup stale buffers
    fn cleanup_stale(&mut self) {
        let timeout = std::time::Duration::from_secs(5);
        self.user_buffers.retain(|_, buffer| !buffer.is_stale(timeout));
    }

    /// Get the number of active audio streams
    pub fn active_streams(&self) -> usize {
        self.user_buffers
            .values()
            .filter(|b| b.available() > 0)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mixer() {
        let mut mixer = AudioMixer::new(48000, 4800);

        // Add audio from two users
        mixer.add_audio(1, &[1000i16; 960]);
        mixer.add_audio(2, &[500i16; 960]);

        // Mix
        let mut output = vec![0i16; 960];
        mixer.mix(&mut output);

        // Output should be sum of both
        assert_eq!(output[0], 1500);
    }
}
