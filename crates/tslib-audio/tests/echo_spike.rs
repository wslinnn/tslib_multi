//! Host simulation spike for the software AEC (阶段 2 前置验证, 二期方案 §3.2).
//!
//! Synthesizes a 48 kHz talkspurt far-end signal, pushes it through a
//! drifting two-tap acoustic path (primary reflection ~108 ms, secondary ~66
//! ms — matching the app's playback watermark + HW latency regime), adds a
//! constant noise floor and a double-talk segment, and runs
//! [`SpeexEchoEngine`] through the same 100 ms reference alignment the app
//! wiring uses.
//!
//! Acceptance (二期方案 §3.2): steady-state ERLE ≥ 15 dB; per-frame CPU is
//! reported with a host sanity bound (< 2 ms). The arm64 budget is measured
//! on device later — this test only proves engine quality on a synthetic
//! loopback.
//!
//! Run with an optimized build, otherwise CPU numbers are meaningless:
//!
//! ```text
//! cargo test --release -p tslib-audio --test echo_spike -- --nocapture
//! ```

use std::collections::VecDeque;
use std::time::Instant;

use tslib_audio::echo::{EchoEngine, SpeexEchoEngine};

const SAMPLE_RATE: usize = 48_000;
const FRAME: usize = 960; // 20 ms @ 48 kHz
const TAIL: usize = 12_000; // 250 ms — same as the app wiring
const ALIGN_FRAMES: usize = 5; // 100 ms static alignment, same as the app wiring

const STATIC_WARMUP_FRAMES: usize = 300; // 6 s adaptation on a fixed path
const STATIC_MEASURE_FRAMES: usize = 500; // 10 s measured — engine convergence
const DRIFT_WARMUP_FRAMES: usize = 150; // 3 s to settle after drift starts
const DRIFT_MEASURE_FRAMES: usize = 500; // 10 s measured — drift tracking
const DOUBLE_TALK_FRAMES: usize = 150; // 3 s local speech
const RECOVERY_FRAMES: usize = 100; // 2 s post-double-talk ERLE recheck

/// Fixed-seed LCG so the scenario is deterministic across runs.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as f64 / (1u64 << 31) as f64) - 1.0 // [-1, 1)
    }
}

/// Two-tap drifting acoustic path fed by a far-sample ring. Returns the echo
/// sample and the true echo energy (for exact ERLE denominators).
struct EchoPath {
    ring: Vec<f64>,
    ring_pos: usize,
    samples_pushed: usize,
    delay1: usize,
    delay2: usize,
}

impl EchoPath {
    /// Primary tap wanders ±4 samples around `base_delay`, stepping once per
    /// 100 ms — slow enough for the adaptive filter to track, fast enough to
    /// prove drift robustness. Secondary tap is a fixed later reflection
    /// (must lie inside the alignment window + tail, i.e. after the primary
    /// tap in the engine's reference timeline — an earlier reflection would
    /// be physically wrong and unrepresentable for the filter).
    fn new(base_delay: usize, delay2: usize) -> Self {
        Self {
            ring: vec![0.0; 16_384],
            ring_pos: 0,
            samples_pushed: 0,
            delay1: base_delay,
            delay2,
        }
    }

    fn push(&mut self, x: f64, base_delay: usize, drift: bool, rng: &mut Lcg) -> (f64, f64) {
        let tap = |ring: &Vec<f64>, pos: usize, d: usize| ring[(pos + ring.len() - d) % ring.len()];
        let y1 = 0.72 * tap(&self.ring, self.ring_pos, self.delay1);
        let y2 = 0.31 * tap(&self.ring, self.ring_pos, self.delay2);
        self.ring[self.ring_pos] = x;
        self.ring_pos = (self.ring_pos + 1) % self.ring.len();
        self.samples_pushed += 1;

        // Bounded random walk of the primary delay, once per 100 ms.
        if drift && self.samples_pushed % (SAMPLE_RATE / 10) == 0 {
            let step = if rng.next() < 0.0 { -1i64 } else { 1 };
            let next = self.delay1 as i64 + step;
            if next >= (base_delay - 4) as i64 && next <= (base_delay + 4) as i64 {
                self.delay1 = next as usize;
            }
        }
        (y1 + y2, y1 * y1 + y2 * y2)
    }
}

/// Speech-like far source: 24 broadband sinusoids under a talkspurt envelope
/// (on ~1.4 s, off ~0.6 s), plus a faint room tone while silent.
struct FarSource {
    phases: Vec<f64>,
    freqs: Vec<f64>,
    t: usize,
    spurt_left: usize,
    talking: bool,
}

impl FarSource {
    fn new(rng: &mut Lcg) -> Self {
        let freqs: Vec<f64> = (0..24).map(|_| 120.0 + 4080.0 * rng.next().abs()).collect();
        let phases: Vec<f64> = (0..24)
            .map(|_| 2.0 * std::f64::consts::PI * rng.next())
            .collect();
        Self {
            phases,
            freqs,
            t: 0,
            spurt_left: ((1.0 + rng.next().abs()) * SAMPLE_RATE as f64) as usize,
            talking: true,
        }
    }

    fn next_sample(&mut self, rng: &mut Lcg) -> f64 {
        if self.spurt_left == 0 {
            self.talking = !self.talking;
            let span = if self.talking { 1.4 } else { 0.6 };
            self.spurt_left = ((span + 0.5 * rng.next().abs()) * SAMPLE_RATE as f64) as usize;
        }
        self.spurt_left -= 1;
        let s = if !self.talking {
            0.03 * rng.next()
        } else {
            // Slow AM for speech-like envelope dynamics.
            let am = 0.6
                + 0.4
                    * (2.0 * std::f64::consts::PI * 2.7 * self.t as f64 / SAMPLE_RATE as f64).sin();
            let mut acc = 0.0;
            for (i, f) in self.freqs.iter().enumerate() {
                acc += (2.0 * std::f64::consts::PI * f * self.t as f64 / SAMPLE_RATE as f64
                    + self.phases[i])
                    .sin();
            }
            am * acc / 8.0 + 0.02 * rng.next()
        };
        self.t += 1;
        s * 3000.0
    }
}

/// Local speech for the double-talk segment: 220 + 880 Hz under a 3 Hz
/// envelope — loud enough to mask, quiet enough not to clip.
fn local_speech(t: usize) -> f64 {
    let env = 0.5 + 0.5 * (2.0 * std::f64::consts::PI * 3.0 * t as f64 / SAMPLE_RATE as f64).sin();
    let s = (2.0 * std::f64::consts::PI * 220.0 * t as f64 / SAMPLE_RATE as f64).sin()
        + 0.5 * (2.0 * std::f64::consts::PI * 880.0 * t as f64 / SAMPLE_RATE as f64).sin();
    env * s * 1400.0
}

fn energy_db(echo_energy: f64, residual_energy: f64) -> f64 {
    10.0 * (echo_energy / residual_energy.max(1.0)).log10()
}

struct PhaseAccum {
    echo_energy: f64,
    out_energy: f64,
}

impl PhaseAccum {
    fn new() -> Self {
        Self {
            echo_energy: 0.0,
            out_energy: 0.0,
        }
    }
    fn add_echo(&mut self, e: f64) {
        self.echo_energy += e;
    }
    fn add_out(&mut self, out: &[i16]) {
        for &s in out {
            self.out_energy += (s as f64) * (s as f64);
        }
    }
}

#[test]
fn echo_spike_synthetic_loopback() {
    let mut rng = Lcg(0x5EED_2026_1103);
    let mut engine = SpeexEchoEngine::new(SAMPLE_RATE as u32, FRAME, TAIL)
        .expect("engine creation must succeed");

    let base_delay = 5_200; // ~108 ms direct path
    let mut path = EchoPath::new(base_delay, 6_800); // + ~34 ms first reflection
    let mut far = FarSource::new(&mut rng);

    let mut ref_fifo: VecDeque<Vec<i16>> = VecDeque::new();
    let mut mic = vec![0i16; FRAME];
    let mut out = vec![0i16; FRAME];

    let mut stat_acc = PhaseAccum::new();
    let mut drift_acc = PhaseAccum::new();
    let mut recovery = PhaseAccum::new();
    let mut dt_speech_energy = 0.0f64;
    let mut dt_out_energy = 0.0f64;
    let mut frame_us: Vec<u128> = Vec::new();

    let static_start = STATIC_WARMUP_FRAMES;
    let static_end = static_start + STATIC_MEASURE_FRAMES;
    let drift_start = static_end;
    let drift_meas_start = drift_start + DRIFT_WARMUP_FRAMES;
    let drift_meas_end = drift_meas_start + DRIFT_MEASURE_FRAMES;
    let dt_end = drift_meas_end + DOUBLE_TALK_FRAMES;
    let total = dt_end + RECOVERY_FRAMES;

    for f in 0..total {
        let drift_on = f >= drift_start;
        // Far frame first (this is the app's playback mix), then the acoustic
        // path turns it into what the microphone hears.
        let mut far_frame = vec![0i16; FRAME];
        for i in 0..FRAME {
            let x = far.next_sample(&mut rng);
            let (echo, echo_e) = path.push(x, base_delay, drift_on, &mut rng);
            far_frame[i] = x.clamp(-32_767.0, 32_767.0) as i16;
            // Constant noise floor like the 纯净采集 source (~-45 dBFS RMS).
            // Note: a perfect canceller leaves this floor as the residual, so
            // it caps the measurable ERLE at ~21-22 dB (10*log10 of echo
            // energy over noise energy). A static reading at that level means
            // the echo is essentially gone, not that the engine stalled.
            let mut m = echo + 60.0 * rng.next();
            let in_dt = f >= drift_meas_end && f < dt_end;
            if in_dt {
                let s = local_speech(f * FRAME + i);
                m += s;
                dt_speech_energy += s * s;
            }
            mic[i] = m.clamp(-32_767.0, 32_767.0) as i16;

            if (static_start..static_end).contains(&f) {
                stat_acc.add_echo(echo_e);
            } else if (drift_meas_start..drift_meas_end).contains(&f) {
                drift_acc.add_echo(echo_e);
            } else if f >= dt_end {
                recovery.add_echo(echo_e);
            }
        }

        // Reference alignment identical to the app wiring: the engine sees
        // the far frame ALIGN_FRAMES behind the mic frame.
        ref_fifo.push_back(far_frame);
        let far_for_engine = if ref_fifo.len() > ALIGN_FRAMES {
            ref_fifo.pop_front().unwrap()
        } else {
            vec![0i16; FRAME]
        };

        let t0 = Instant::now();
        engine.process(&mic, &far_for_engine, &mut out).unwrap();
        let elapsed = t0.elapsed().as_micros();
        if (drift_meas_start..drift_meas_end).contains(&f) {
            frame_us.push(elapsed);
        }

        if (static_start..static_end).contains(&f) {
            stat_acc.add_out(&out);
        } else if (drift_meas_start..drift_meas_end).contains(&f) {
            drift_acc.add_out(&out);
        } else if f >= dt_end {
            recovery.add_out(&out);
        } else if (drift_meas_end..dt_end).contains(&f) {
            dt_out_energy += out.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>();
        }
    }

    let erle_static = energy_db(stat_acc.echo_energy, stat_acc.out_energy);
    let erle_drift = energy_db(drift_acc.echo_energy, drift_acc.out_energy);
    let erle_recovery = energy_db(recovery.echo_energy, recovery.out_energy);
    let avg_us = frame_us.iter().sum::<u128>() / frame_us.len() as u128;
    let mut sorted = frame_us.clone();
    sorted.sort_unstable();
    let p95_us = sorted[(sorted.len() as f64 * 0.95) as usize];
    let max_us = *sorted.last().unwrap();
    let dt_ratio = (dt_out_energy / dt_speech_energy.max(1.0)).sqrt();

    println!(
        "echo_spike: {:4} frames = {:.1}s simulated (tail {TAIL}, align {ALIGN_FRAMES})",
        total,
        total as f64 * FRAME as f64 / SAMPLE_RATE as f64
    );
    println!(
        "echo_spike: ERLE static   = {erle_static:6.1} dB   (engine convergence, target >= 20 dB)"
    );
    println!("echo_spike: ERLE drifting = {erle_drift:6.1} dB   (acceptance, target >= 15 dB)");
    println!(
        "echo_spike: ERLE recovery = {erle_recovery:6.1} dB   (post double-talk, informational)"
    );
    println!(
        "echo_spike: CPU per frame = avg {avg_us}us  p95 {p95_us}us  max {max_us}us  (host sanity < 2000us; arm64 budget measured on device)"
    );
    println!(
        "echo_spike: double-talk out/speech RMS ratio = {dt_ratio:.2} (must keep speech, not suppress mic)"
    );

    // Engine quality on a fixed path (informational bar, well above the
    // acceptance target — a synthetic loopback should cancel deeply).
    assert!(
        erle_static >= 20.0,
        "static-path ERLE {erle_static:.1} dB below the 20 dB engine bar"
    );
    // Headline acceptance from 二期方案 §3.2 (drifting path).
    assert!(
        erle_drift >= 15.0,
        "drifting-path ERLE {erle_drift:.1} dB below the 15 dB acceptance bar"
    );
    // Host sanity bound; debug builds don't get meaningful timings.
    if !cfg!(debug_assertions) {
        assert!(p95_us < 2_000, "host p95 {p95_us}us exceeds sanity bound");
    }
    // The engine must cancel echo, not the microphone: after the double-talk
    // segment the residual must still carry the bulk of the local speech.
    assert!(
        dt_ratio > 0.3,
        "double-talk suppressed local speech too aggressively (out/speech RMS ratio {dt_ratio:.2})"
    );
}
