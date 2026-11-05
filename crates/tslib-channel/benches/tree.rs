//! Simple deterministic benchmark (no external harness).
//!
//! Run with `cargo bench -p tslib-channel --bench tree`.

use std::hint::black_box;
use std::time::Instant;
use tslib_core::state::Channel;
use tslib_channel::ChannelTree;

fn channels(count: usize) -> Vec<Channel> {
    (0..count)
        .map(|index| {
            let id = index as u64 + 1;
            let parent = if index < 10 {
                0
            } else {
                (index % 10) as u64 + 1
            };
            Channel {
                id,
                parent_id: parent,
                name: format!("channel-{id}"),
                order: if index % 10 == 0 { 0 } else { id - 1 },
                ..Default::default()
            }
        })
        .collect()
}

fn main() {
    let input = channels(200);
    let iterations = 1000;

    let start = Instant::now();
    for _ in 0..iterations {
        let tree = ChannelTree::from_channels(black_box(input.clone()));
        black_box(tree.len());
    }
    let build = start.elapsed() / iterations;

    let tree = ChannelTree::from_channels(input);
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(tree.path_to(black_box(200)));
        black_box(tree.effective_max_clients(black_box(200)));
    }
    let query = start.elapsed() / iterations;

    println!("tree_build_200ch: {build:?}/iter");
    println!("tree_path_and_limit: {query:?}/iter");
}
