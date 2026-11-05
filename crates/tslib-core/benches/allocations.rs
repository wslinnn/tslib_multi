//! Allocation-count benchmark for event cloning (PR-02).
//!
//! Run with `cargo bench -p tslib-core --bench allocations`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tslib_core::{AudioCodec, Event};

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
    let event = Event::AudioReceived {
        user_id: 1,
        codec: AudioCodec::OpusVoice,
        data: Arc::from(vec![0u8; 80]),
        seq: 1,
        is_whisper: false,
    };
    let iterations = 100_000u64;

    let before = ALLOCATIONS.load(Ordering::Relaxed);
    for _ in 0..iterations {
        black_box(black_box(&event).clone());
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;

    println!(
        "audio_event_clone_allocations: {:.4} per clone",
        allocations as f64 / iterations as f64
    );
}
