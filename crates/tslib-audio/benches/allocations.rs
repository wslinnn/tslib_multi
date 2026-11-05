//! Allocation-count benchmark for the audio callbacks (PR-09).
//!
//! Run with `cargo bench -p tslib-audio --bench allocations`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};

struct Counting;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout)
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.realloc(pointer, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn main() {
    use tslib_audio::mixer::AudioMixer;

    let mut mixer = AudioMixer::new(48_000, 4_800);
    mixer.add_audio(1, &[1000i16; 960]);
    let mut output = vec![0i16; 960];
    let iterations = 10_000u64;

    // Warm up (temp buffer allocation).
    mixer.mix(&mut output);

    let before = ALLOCATIONS.load(Ordering::Relaxed);
    for _ in 0..iterations {
        mixer.mix(black_box(&mut output));
    }
    let mixer_allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;

    println!(
        "mixer_allocations_per_call: {:.4}",
        mixer_allocations as f64 / iterations as f64
    );
}
