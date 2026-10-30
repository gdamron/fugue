//! The audio thread's output taps, on a graph driven block by block: a
//! module whose outputs count the samples, so every bin's bounds are known.

use indexmap::IndexMap;

use super::*;
use crate::alloc_counter::allocator_events;
use crate::invention::graph::MasterObservers;
use crate::invention::publish::TopologyMirror;
use crate::{GraphModule, Module, MAX_BLOCK};

const OUTPUTS: [&str; 2] = ["up", "down"];

/// Outputs the sample number on `up` and its negation on `down`.
struct Count {
    sample: u64,
    out: [[f32; MAX_BLOCK]; 2],
}

impl Module for Count {
    fn name(&self) -> &str {
        "count"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            let n = (self.sample + i as u64) as f32;
            self.out[0][i] = n;
            self.out[1][i] = -n;
        }
        self.sample += frames as u64;
        true
    }

    fn inputs(&self) -> &[&str] {
        &[]
    }

    fn outputs(&self) -> &[&str] {
        &OUTPUTS
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut []
    }

    fn output_block(&self, index: usize) -> &[f32] {
        &self.out[index]
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("no input {port}"))
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let index = OUTPUTS.iter().position(|p| *p == port).ok_or("no port")?;
        Ok(self.out[index][0])
    }
}

/// A graph running a counter as `count`, its taps linked as a live
/// graph's are, and the counter's instance key.
fn tapped() -> (SignalGraph, Arc<TapBoard>, u32) {
    let mut modules = IndexMap::new();
    let count = Count {
        sample: 0,
        out: [[0.0; MAX_BLOCK]; 2],
    };
    modules.insert("count".to_string(), GraphModule::Module(Box::new(count)));
    let mut graph = SignalGraph::new(modules, Vec::new(), Vec::new(), MasterObservers::default());
    graph.recompile();
    let mirror = TopologyMirror::of(&graph.modules, &graph.edges);
    let board = TapBoard::new();
    graph.output_taps = Some(OutputTaps::new(Arc::clone(&board), mirror.instance_keys()));
    (graph, board, mirror.modules["count"].instance)
}

/// Renders `samples` in blocks of `block`.
fn render(graph: &mut SignalGraph, samples: u64, block: usize) {
    let mut left = [0.0f32; MAX_BLOCK];
    let mut right = [0.0f32; MAX_BLOCK];
    let mut done = 0;
    while done < samples {
        let n = (samples - done).min(block as u64) as usize;
        graph.process_block(&mut left[..n], &mut right[..n]);
        done += n as u64;
    }
}

fn drain(board: &TapBoard, slot: usize) -> Vec<TapBin> {
    std::iter::from_fn(|| board.slots[slot].ring.pop()).collect()
}

#[test]
fn bins_hold_the_lowest_and_highest_value_of_each_run_across_blocks() {
    let (mut graph, board, key) = tapped();
    render(&mut graph, 100, 64);
    assert!(
        drain(&board, 0).is_empty(),
        "nothing is tapped before a tap opens"
    );

    board.open(0, 1, key, [0, 1]);
    // Blocks of 48 split bins across blocks, and a segment ends mid-bin.
    render(&mut graph, 48 * 10, 48);
    let bins = drain(&board, 0);
    assert_eq!(bins.len(), 48 * 10 / TAP_BIN);
    for (n, bin) in bins.iter().enumerate() {
        let start = 100 + (n * TAP_BIN) as u64;
        let last = (start + TAP_BIN as u64 - 1) as f32;
        assert_eq!(bin.serial, 1);
        assert_eq!(bin.start, start);
        assert_eq!(bin.min, [start as f32, -last]);
        assert_eq!(bin.max, [last, -(start as f32)]);
    }
}

#[test]
fn one_port_fills_one_channel_and_closing_stops_the_bins() {
    let (mut graph, board, key) = tapped();
    board.open(3, 7, key, [1, usize::MAX]);
    render(&mut graph, 64, 64);
    let bins = drain(&board, 3);
    assert_eq!(bins.len(), 2);
    assert_eq!((bins[1].min[0], bins[1].max[0]), (-63.0, -32.0));

    board.close(3);
    render(&mut graph, 640, 64);
    assert!(drain(&board, 3).is_empty());
    // Opened again, the slot starts a fresh bin under the new serial.
    board.open(3, 8, key, [0, usize::MAX]);
    render(&mut graph, 40, 40);
    render(&mut graph, 24, 24);
    let bins = drain(&board, 3);
    assert_eq!(bins.len(), 2);
    assert_eq!((bins[0].serial, bins[0].start), (8, 704));
}

#[test]
fn the_board_knows_once_its_graph_has_gone() {
    let (graph, board, _) = tapped();
    assert!(board.running());
    drop(graph);
    assert!(!board.running());
}

#[test]
fn a_tap_on_an_instance_not_running_sends_nothing() {
    let (mut graph, board, key) = tapped();
    board.open(0, 1, key + 1_000, [0, 1]);
    board.open(1, 2, key, [5, usize::MAX]);
    render(&mut graph, 640, 64);
    assert!(drain(&board, 0).is_empty(), "no module has that instance");
    assert!(drain(&board, 1).is_empty(), "the module has no port 5");
}

#[test]
fn a_full_ring_drops_bins_and_the_next_one_sent_shows_the_gap() {
    let (mut graph, board, key) = tapped();
    board.open(0, 1, key, [0, 1]);
    render(&mut graph, ((TAP_BIN_CAPACITY + 10) * TAP_BIN) as u64, 64);
    assert_eq!(drain(&board, 0).len(), TAP_BIN_CAPACITY);
    render(&mut graph, TAP_BIN as u64, 64);
    let next = drain(&board, 0);
    assert_eq!(next[0].start, ((TAP_BIN_CAPACITY + 10) * TAP_BIN) as u64);
}

#[test]
fn tapping_neither_allocates_nor_frees_on_the_audio_thread() {
    let (mut graph, board, key) = tapped();
    render(&mut graph, 64, 64);
    let mut left = [0.0f32; 64];
    let mut right = [0.0f32; 64];
    let ((), allocs, frees) = allocator_events(|| {
        // Closed, every slot open, and opening and closing as it plays.
        for _ in 0..100 {
            graph.process_block(&mut left, &mut right);
        }
        for slot in 0..MAX_OUTPUT_TAPS {
            board.open(slot, slot as u32 + 1, key, [0, 1]);
        }
        for block in 0..300u32 {
            graph.process_block(&mut left, &mut right);
            let slot = block as usize % MAX_OUTPUT_TAPS;
            if block % 3 == 0 {
                board.close(slot);
                board.open(slot, 100 + block, key, [1, usize::MAX]);
            }
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(!drain(&board, 0).is_empty());
}

/// What the taps cost the audio thread a block: closed, with one open and
/// with all of them open. Run in release for the numbers the docs state
/// (`cargo test --release --lib tapping_costs -- --nocapture`); in any
/// build it is a tripwire, its budget a hundredth of the block's real-time
/// duration, as the master observers' is.
///
/// Each figure is the cheapest of many short batches, so a batch a
/// loaded machine preempted does not count against the taps. The rings
/// are emptied between batches, outside the timing, as a view reading
/// them keeps them.
#[test]
fn tapping_costs_a_sliver_of_a_block() {
    use std::hint::black_box;
    use std::time::{Duration, Instant};

    const FRAMES: usize = 128;
    const BATCHES: u32 = 200;
    const BLOCKS_PER_BATCH: u32 = 100;
    let budget = Duration::from_secs_f64(FRAMES as f64 / 48_000.0 / 100.0);
    let (mut graph, board, key) = tapped();
    graph.set_block_size(FRAMES);
    render(&mut graph, FRAMES as u64, FRAMES);
    let per_block = |graph: &mut SignalGraph| {
        (0..BATCHES)
            .map(|_| {
                let start = Instant::now();
                for _ in 0..BLOCKS_PER_BATCH {
                    black_box(&mut *graph).tap_outputs(black_box(FRAMES));
                }
                let elapsed = start.elapsed() / BLOCKS_PER_BATCH;
                for slot in 0..MAX_OUTPUT_TAPS {
                    drain(&board, slot);
                }
                elapsed
            })
            .min()
            .unwrap()
    };

    let closed = per_block(&mut graph);
    board.open(0, 1, key, [0, 1]);
    let one = per_block(&mut graph);
    for slot in 1..MAX_OUTPUT_TAPS {
        board.open(slot, slot as u32 + 1, key, [0, 1]);
    }
    let all = per_block(&mut graph);
    eprintln!(
        "output taps, a {FRAMES}-frame block: closed {closed:?}, one open {one:?}, \
         {MAX_OUTPUT_TAPS} open {all:?}"
    );
    assert!(closed < budget / 100, "closed taps took {closed:?} a block");
    assert!(one < budget, "one tap took {one:?} a block");
    assert!(
        all < budget * MAX_OUTPUT_TAPS as u32,
        "all took {all:?} a block"
    );
}
