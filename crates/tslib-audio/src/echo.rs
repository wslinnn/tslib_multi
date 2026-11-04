//! Software acoustic echo cancellation (AEC).
//!
//! Removes loudspeaker echo from the microphone signal by adaptively
//! estimating the acoustic path between the playback reference (the exact PCM
//! written to `AudioTrack`) and the capture. Built on the vendored speexdsp
//! frequency-domain multichannel filter (`mdf.c`, BSD license, see
//! `speexdsp/README.md`).
//!
//! The engine is stateful and not thread-safe: create one per capture
//! session, call [`EchoEngine::process`] once per frame from a single thread,
//! and [`EchoEngine::reset`] when the far reference timeline breaks (route
//! change, track rebuild). Processing must keep running on every frame —
//! including muted or locally silent ones — so the adaptive filter stays
//! converged; the mute gate lives above this layer.
//!
//! ```rust,no_run
//! use tslib_audio::echo::{EchoEngine, SpeexEchoEngine};
//!
//! # fn main() -> anyhow::Result<()> {
//! // 48 kHz, 960 samples (20 ms), 12000-sample tail (~250 ms of path).
//! let mut engine = SpeexEchoEngine::new(48_000, 960, 12_000)?;
//!
//! let mic: Vec<i16> = vec![0; 960];   // raw capture frame
//! let far: Vec<i16> = vec![0; 960];   // playback mix frame, same timeline
//! let mut out = vec![0i16; 960];
//! engine.process(&mic, &far, &mut out)?;
//! # Ok(())
//! # }
//! ```

use crate::error::{AudioError, Result};

/// Raw speexdsp bindings (symbols come from the `speexdsp_echo` static
/// library built in `build.rs`).
#[repr(C)]
struct SpeexEchoState {
    _private: [u8; 0],
}

extern "C" {
    fn speex_echo_state_init(frame_size: i32, filter_length: i32) -> *mut SpeexEchoState;
    fn speex_echo_state_destroy(st: *mut SpeexEchoState);
    fn speex_echo_state_reset(st: *mut SpeexEchoState);
    /// `rec` = microphone capture, `play` = playback reference, `out` = echo-
    /// cancelled output. All three are `frame_size` samples.
    fn speex_echo_cancellation(
        st: *mut SpeexEchoState,
        rec: *const i16,
        play: *const i16,
        out: *mut i16,
    );
    fn speex_echo_ctl(st: *mut SpeexEchoState, request: i32, ptr: *mut std::ffi::c_void) -> i32;
}

/// `SPEEX_ECHO_SET_SAMPLING_RATE` from `speex_echo.h`. The state defaults to
/// 8 kHz, which calibrates the DC-notch width and the leak/beta estimators
/// for the wrong rate — at 48 kHz that audibly eats low microphone content
/// and skews the adaptation statistics, so the rate must always be set.
const SPEEX_ECHO_SET_SAMPLING_RATE: i32 = 24;

/// Engine-agnostic echo cancellation contract so alternative engines
/// (e.g. a model-based AEC later) can be swapped in without touching the
/// caller or the JNI surface.
pub trait EchoEngine: Send {
    /// Cancel the echo of `far` contained in `mic`, writing `out`.
    ///
    /// All three slices must be exactly one frame (`frame_samples` samples,
    /// matching lengths). Returns the number of samples written to `out`.
    ///
    /// Call for every frame regardless of mute state so the adaptive filter
    /// keeps converging; bypass only on [`Err`] (pass `mic` through — an
    /// audible echo is better than swallowed speech).
    fn process(&mut self, mic: &[i16], far: &[i16], out: &mut [i16]) -> Result<usize>;

    /// Discard the learned path estimate (e.g. after a route change that
    /// breaks the reference timeline). Convergence restarts from scratch.
    fn reset(&mut self) -> Result<()>;
}

/// speexdsp-based [`EchoEngine`].
///
/// Holds one `SpeexEchoState` (roughly a few hundred KB for a 250 ms tail @
/// 48 kHz). Send but not Sync: all calls must come from one thread (the
/// capture loop in the app).
pub struct SpeexEchoEngine {
    st: *mut SpeexEchoState,
    sample_rate: u32,
    frame_samples: usize,
    tail_samples: usize,
}

// The raw state pointer is only ever touched through `&mut self`, so exclusive
// access is already guaranteed by the borrow checker; one engine per session
// never migrates across threads mid-call in our usage.
unsafe impl Send for SpeexEchoEngine {}

/// Near-clipping threshold used by the blow-up guard: 32 kHz-scale amplitude
/// in a 16-bit frame is loudspeaker-level, not residual.
const NEAR_CLIP: i16 = 32_700;

/// Fraction of the output frame allowed to sit at near-clip when the mic
/// itself never clips there — beyond this the engine is diverged and the
/// frame is zeroed instead of forwarded (a digital scream is worse than a
/// brief dropout).
const BLOWUP_RATIO: usize = 3; // 1/3 of the frame

/// Blow-up guard shared by every [`EchoEngine`] implementation: if the engine
/// produced a near-clipped frame while the mic was nowhere near clipping, the
/// adaptation diverged — zero the frame (aligned with the zero-fill contract
/// of the rest of the audio pipeline) instead of forwarding a digital scream.
/// Blow-up guard for echo engine output: AEC instability shows up as output
/// slamming the rails on an otherwise clean mic frame, which users hear as
/// loud crackling; those frames are zeroed instead.
///
/// `gain` is the fixed output gain the engine applies after its processing
/// (e.g. the AEC3 engine's post-gain). The mic-side cleanliness threshold is
/// scaled by it so that genuine loud speech lifted into the clip region by
/// the gain is not misclassified as engine blow-up (only frames the mic
/// itself would drive near-clip at unity gain are spared).
pub fn sanitize_echo_output(out: &mut [i16], mic: &[i16], gain: f32) {
    let clipped_out = out.iter().filter(|&&s| s.abs() >= NEAR_CLIP).count();
    if clipped_out * BLOWUP_RATIO > out.len() {
        let near_clip = NEAR_CLIP as f32 / gain;
        let clipped_mic = mic
            .iter()
            .filter(|&&s| s.abs() as f32 >= near_clip)
            .count();
        if clipped_mic == 0 {
            tracing::warn!(
                "echo engine blow-up guard: {}/{} output samples near-clip on a clean mic frame, zeroing frame",
                clipped_out,
                out.len()
            );
            out.fill(0);
        }
    }
}

impl SpeexEchoEngine {
    /// Create an engine for `sample_rate` Hz, `frame_samples`-sample frames
    /// and a filter tail of `tail_samples` (echo paths slower than that
    /// cannot be cancelled; the app uses 250 ms @ 48 kHz). The tail dominates
    /// the memory footprint.
    pub fn new(sample_rate: u32, frame_samples: usize, tail_samples: usize) -> Result<Self> {
        if sample_rate == 0 || frame_samples == 0 || tail_samples == 0 {
            return Err(AudioError::InvalidConfig(format!(
                "echo engine: sample_rate({sample_rate}), frame_samples({frame_samples}) and tail_samples({tail_samples}) must be non-zero"
            )));
        }
        if sample_rate > i32::MAX as u32
            || frame_samples > i32::MAX as usize
            || tail_samples > i32::MAX as usize
        {
            return Err(AudioError::InvalidConfig(
                "echo engine: sizes exceed int32 range".to_string(),
            ));
        }
        let st = unsafe { speex_echo_state_init(frame_samples as i32, tail_samples as i32) };
        if st.is_null() {
            return Err(AudioError::DeviceInit(
                "speex_echo_state_init returned null".to_string(),
            ));
        }
        let mut rate = sample_rate as i32;
        unsafe {
            speex_echo_ctl(
                st,
                SPEEX_ECHO_SET_SAMPLING_RATE,
                &mut rate as *mut i32 as *mut std::ffi::c_void,
            );
        }
        Ok(Self {
            st,
            sample_rate,
            frame_samples,
            tail_samples,
        })
    }

    /// Sample rate this engine was created for.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Frame size in samples this engine was created for.
    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    /// Filter tail length in samples.
    pub fn tail_samples(&self) -> usize {
        self.tail_samples
    }

    /// Blow-up guard: see [`sanitize_echo_output`].
    fn sanitize(&self, out: &mut [i16], mic: &[i16]) {
        sanitize_echo_output(out, mic, 1.0);
    }
}

impl EchoEngine for SpeexEchoEngine {
    fn process(&mut self, mic: &[i16], far: &[i16], out: &mut [i16]) -> Result<usize> {
        if mic.len() != self.frame_samples
            || far.len() != self.frame_samples
            || out.len() != self.frame_samples
        {
            return Err(AudioError::InvalidConfig(format!(
                "echo engine: frame mismatch (mic={}, far={}, out={}, expected={})",
                mic.len(),
                far.len(),
                out.len(),
                self.frame_samples
            )));
        }
        unsafe {
            speex_echo_cancellation(self.st, mic.as_ptr(), far.as_ptr(), out.as_mut_ptr());
        }
        self.sanitize(out, mic);
        Ok(out.len())
    }

    fn reset(&mut self) -> Result<()> {
        unsafe { speex_echo_state_reset(self.st) };
        Ok(())
    }
}

impl Drop for SpeexEchoEngine {
    fn drop(&mut self) {
        unsafe { speex_echo_state_destroy(self.st) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_sizes() {
        assert!(SpeexEchoEngine::new(0, 960, 12_000).is_err());
        assert!(SpeexEchoEngine::new(48_000, 0, 12_000).is_err());
        assert!(SpeexEchoEngine::new(48_000, 960, 0).is_err());
    }

    #[test]
    fn rejects_frame_mismatch() {
        let mut engine = SpeexEchoEngine::new(48_000, 64, 1920).unwrap();
        let mic = vec![0i16; 64];
        let far = vec![0i16; 32];
        let mut out = vec![0i16; 64];
        assert!(engine.process(&mic, &far, &mut out).is_err());

        let far = vec![0i16; 64];
        let mut out = vec![0i16; 63];
        assert!(engine.process(&mic, &far, &mut out).is_err());
    }

    /// Contract the app relies on for the headset case: with a silent far
    /// reference there is nothing to subtract and the mic must come through
    /// (natural bypass, zero regression). The engine still applies its DC
    /// blocker, so after the notch settles the output tracks the input
    /// closely — the assertion is a waveform correlation, not exactness.
    #[test]
    fn silent_far_is_passthrough() {
        const N: usize = 960;
        const FRAMES: usize = 8; // let the DC notch reach steady state
        let mut engine = SpeexEchoEngine::new(48_000, N, 4800).unwrap();
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

    /// Reset must not corrupt the engine: process after reset still works.
    #[test]
    fn reset_then_process() {
        const N: usize = 64;
        let mut engine = SpeexEchoEngine::new(48_000, N, 256).unwrap();
        let mic = vec![100i16; N];
        let far = vec![50i16; N];
        let mut out = vec![0i16; N];
        engine.reset().unwrap();
        assert_eq!(engine.process(&mic, &far, &mut out).unwrap(), N);
    }
}
