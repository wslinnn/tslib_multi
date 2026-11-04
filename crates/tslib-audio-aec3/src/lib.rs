//! WebRTC AEC3 echo cancellation engine.
//!
//! Implements [`tslib_audio::echo::EchoEngine`] on top of the vendored
//! freedesktop `webrtc-audio-processing` 2.1 (see `c/wap/README.md` for
//! provenance and licensing). This is the engine on the app's capture path,
//! replacing speexdsp MDF: AEC3 ships its own delay estimation — no
//! caller-side alignment constant — plus residual echo suppression, which
//! speex MDF lacks entirely.
//!
//! Frame contract: identical to the speex engine (`process(mic, far, out)`
//! per 20 ms frame). Internally the frame is split into 10 ms chunks —
//! AEC3's native granularity — with render fed before its paired capture
//! chunk. `tail_samples` is accepted for signature parity with
//! [`tslib_audio::echo::SpeexEchoEngine`] (and the JNI surface) but unused:
//! AEC3 sizes its internal filter from the configuration, not from a caller
//! tail length.
//!
//! The stream must be fed in order (playback mix, then capture); the
//! timeline bookkeeping stays in the app's `EchoReference` FIFO — only the
//! alignment constant disappears, the reference is now a plain in-order
//! stream.

use std::ffi::c_void;

use tslib_audio::echo::{EchoEngine, sanitize_echo_output};
use tslib_audio::error::{AudioError, Result};

extern "C" {
    fn tslib_aec3_create(sample_rate: i32) -> *mut c_void;
    fn tslib_aec3_process_render(handle: *mut c_void, far_ref: *const i16, samples: i32) -> i32;
    fn tslib_aec3_process_capture(
        handle: *mut c_void,
        mic: *const i16,
        out: *mut i16,
        samples: i32,
    ) -> i32;
    fn tslib_aec3_destroy(handle: *mut c_void);
    fn tslib_aec3_post_gain() -> f32;
}

/// AEC3-based [`EchoEngine`]. Send but not Sync: all calls must come from
/// one thread (the capture loop in the app).
pub struct Aec3EchoEngine {
    handle: *mut c_void,
    sample_rate: u32,
    frame_samples: usize,
    chunk_samples: usize, // 10 ms — AEC3's native chunk
    post_gain: f32,       // fixed output gain applied inside the APM after AEC
}

// Same exclusivity argument as the speex engine: the raw handle is only ever
// touched through `&mut self`.
unsafe impl Send for Aec3EchoEngine {}

impl Aec3EchoEngine {
    /// Create an engine for `sample_rate` Hz and `frame_samples`-sample
    /// frames (must be a multiple of the 10 ms chunk). `tail_samples` is
    /// accepted but unused — see the crate docs.
    pub fn new(sample_rate: u32, frame_samples: usize, _tail_samples: usize) -> Result<Self> {
        if !matches!(sample_rate, 16_000 | 32_000 | 48_000) {
            return Err(AudioError::InvalidConfig(format!(
                "aec3 engine: sample_rate {sample_rate} not in 16k/32k/48k"
            )));
        }
        let chunk_samples = (sample_rate / 100) as usize;
        if frame_samples == 0 || frame_samples % chunk_samples != 0 {
            return Err(AudioError::InvalidConfig(format!(
                "aec3 engine: frame_samples {frame_samples} must be a non-zero multiple of the 10 ms chunk ({chunk_samples})"
            )));
        }
        let handle = unsafe { tslib_aec3_create(sample_rate as i32) };
        if handle.is_null() {
            return Err(AudioError::DeviceInit(
                "tslib_aec3_create returned null".to_string(),
            ));
        }
        Ok(Self {
            handle,
            sample_rate,
            frame_samples,
            chunk_samples,
            post_gain: unsafe { tslib_aec3_post_gain() },
        })
    }

    /// Sample rate this engine was created for.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Fixed post-gain the engine applies to the capture output after AEC.
    /// Test and measurement code normalizes output by it so level metrics
    /// describe echo suppression, not the level lift itself.
    pub fn post_gain_factor(&self) -> f32 {
        self.post_gain
    }

    /// Frame size in samples this engine was created for.
    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }
}

impl EchoEngine for Aec3EchoEngine {
    fn process(&mut self, mic: &[i16], far: &[i16], out: &mut [i16]) -> Result<usize> {
        if mic.len() != self.frame_samples
            || far.len() != self.frame_samples
            || out.len() != self.frame_samples
        {
            return Err(AudioError::InvalidConfig(format!(
                "aec3 engine: frame mismatch (mic={}, far={}, out={}, expected={})",
                mic.len(),
                far.len(),
                out.len(),
                self.frame_samples
            )));
        }
        let chunk = self.chunk_samples;
        for i in (0..self.frame_samples).step_by(chunk) {
            let status = unsafe {
                tslib_aec3_process_render(self.handle, far[i..].as_ptr(), chunk as i32)
            };
            if status != 0 {
                return Err(AudioError::DeviceInit(format!(
                    "aec3 ProcessReverseStream failed with {status}"
                )));
            }
            let status = unsafe {
                tslib_aec3_process_capture(
                    self.handle,
                    mic[i..].as_ptr(),
                    out[i..].as_mut_ptr(),
                    chunk as i32,
                )
            };
            if status != 0 {
                return Err(AudioError::DeviceInit(format!(
                    "aec3 ProcessStream failed with {status}"
                )));
            }
        }
        sanitize_echo_output(out, mic, self.post_gain);
        Ok(out.len())
    }

    /// APM exposes no state reset; recreate the canceller (cheap, and this
    /// path only runs on route changes / session restarts).
    fn reset(&mut self) -> Result<()> {
        unsafe { tslib_aec3_destroy(self.handle) };
        self.handle = unsafe { tslib_aec3_create(self.sample_rate as i32) };
        if self.handle.is_null() {
            return Err(AudioError::DeviceInit(
                "tslib_aec3_create returned null on reset".to_string(),
            ));
        }
        Ok(())
    }
}

impl Drop for Aec3EchoEngine {
    fn drop(&mut self) {
        unsafe { tslib_aec3_destroy(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_rates_and_frames() {
        assert!(Aec3EchoEngine::new(8_000, 960, 12_000).is_err());
        assert!(Aec3EchoEngine::new(48_000, 0, 12_000).is_err());
        // 20 ms @ 48 kHz = 960 = 2 x 480: valid. 971 is not chunk-aligned.
        assert!(Aec3EchoEngine::new(48_000, 971, 12_000).is_err());
    }

    #[test]
    fn create_process_reset_roundtrip() {
        const N: usize = 960;
        let mut engine = Aec3EchoEngine::new(48_000, N, 12_000).expect("engine creation");
        let mic = vec![100i16; N];
        let far = vec![50i16; N];
        let mut out = vec![0i16; N];
        assert_eq!(engine.process(&mic, &far, &mut out).unwrap(), N);
        engine.reset().expect("reset");
        assert_eq!(engine.process(&mic, &far, &mut out).unwrap(), N);
    }

    /// Contract the app relies on for the headset case: with a silent far
    /// reference there is nothing to subtract and the mic must come through
    /// (natural bypass, zero regression).
    #[test]
    fn silent_far_is_passthrough() {
        const N: usize = 960;
        const FRAMES: usize = 8;
        let mut engine = Aec3EchoEngine::new(48_000, N, 12_000).unwrap();
        let far = vec![0i16; N];
        let mut out = vec![0i16; N];
        let mut mic = vec![0i16; N];
        for f in 0..FRAMES {
            for (i, m) in mic.iter_mut().enumerate() {
                let t = f * N + i;
                *m = (1000.0 * (2.0 * std::f64::consts::PI * 220.0 * t as f64 / 48_000.0).sin())
                    as i16;
            }
            let written = engine.process(&mic, &far, &mut out).unwrap();
            assert_eq!(written, N);
        }
        let peak_in = mic.iter().map(|s| s.abs()).max().unwrap() as f64;
        let peak_out = out.iter().map(|s| s.abs()).max().unwrap() as f64;
        println!("passthrough: peak in={peak_in} out={peak_out}");
        assert!(
            peak_out > 0.5 * peak_in,
            "mic was attenuated by the idle engine"
        );
    }

    /// The static +6 dB capture post-gain must actually be applied: a quiet
    /// mic-level input comes out at twice its amplitude (within tolerance for
    /// the AEC3-forced high-pass). Guards against ApplyConfig silently
    /// dropping the capture_level_adjustment config (which would leave the
    /// gain at 1.0 and every device quiet again).
    #[test]
    fn post_gain_lifts_quiet_capture() {
        const N: usize = 960;
        const FRAMES: usize = 8;
        let mut engine = Aec3EchoEngine::new(48_000, N, 12_000).unwrap();
        let gain = engine.post_gain_factor();
        assert!((gain - 2.0).abs() < 1e-6, "unexpected pre-gain {gain}");
        let far = vec![0i16; N];
        let mut out = vec![0i16; N];
        let mut mic = vec![0i16; N];
        for f in 0..FRAMES {
            for (i, m) in mic.iter_mut().enumerate() {
                let t = f * N + i;
                *m = (1000.0 * (2.0 * std::f64::consts::PI * 220.0 * t as f64 / 48_000.0).sin())
                    as i16;
            }
            engine.process(&mic, &far, &mut out).unwrap();
        }
        let peak_in = mic.iter().map(|s| s.abs()).max().unwrap() as f64;
        let peak_out = out.iter().map(|s| s.abs()).max().unwrap() as f64;
        println!("pre-gain: peak in={peak_in} out={peak_out} ratio={:.2}", peak_out / peak_in);
        assert!(
            peak_out > 1.6 * peak_in && peak_out < 2.5 * peak_in,
            "capture output not lifted by the configured pre-gain (ratio={:.2})",
            peak_out / peak_in
        );
    }
}
