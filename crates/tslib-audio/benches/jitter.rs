//! Simple deterministic jitter-buffer benchmark (no external harness).
//!
//! Run with `cargo bench -p tslib-audio --bench jitter`.

use std::hint::black_box;
use std::time::{Duration, Instant};
use tslib_audio::config::JitterBufferConfig;
use tslib_audio::{JitterBuffer, JitterOutput};

fn main() {
    let config = JitterBufferConfig::default();
    let mut buffer = JitterBuffer::new(config);
    let start = Instant::now();
    let now = Instant::now();
    let packets = 10_000u16;

    for index in 0..packets {
        let at = now + Duration::from_millis(index as u64 * 20);
        buffer.push(index, vec![0u8; 80], at);
        for output in buffer.pump(at) {
            match output {
                JitterOutput::Packet(data) => {
                    let _ = black_box(data.len());
                }
                JitterOutput::Plc => {
                    let _ = black_box(0usize);
                }
            }
        }
    }

    let elapsed = start.elapsed();
    println!(
        "jitter_push_pump: {:?}/packet ({} packets)",
        elapsed / packets as u32,
        packets
    );
}
