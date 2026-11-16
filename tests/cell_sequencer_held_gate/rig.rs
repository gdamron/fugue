//! A one-cell `CellSequencer` driven sample by sample, and gate checks.

use std::ops::Range;

use fugue::modules::{CellSequencer, Step};
use fugue::Module;

/// One step is 100 samples at 1 kHz.
pub const RATE: u32 = 1_000;
pub const D: usize = 100;
/// The clock input stays high this long after each edge.
const PULSE: usize = 10;

/// What the sequencer put out after processing each sample.
pub struct Trace {
    pub gate: Vec<f32>,
    pub frequency: Vec<f32>,
}

/// A cell sequencer playing `cell` (root 60, default gate 0.5) as its only
/// cell, with on-beat grace placement when `on_beat`.
pub fn sequencer(cell: Vec<Step>, on_beat: bool) -> CellSequencer {
    CellSequencer::new(RATE)
        .with_root_note(60)
        .with_step_count(cell.len())
        .with_gate_length(0.5)
        .with_grace_on_beat(on_beat)
        .with_cells(vec![cell])
}

/// Plays `seq` for `total` samples with clock rising edges on the samples
/// in `edges`.
pub fn drive(seq: &mut CellSequencer, edges: &[usize], total: usize) -> Trace {
    let mut trace = Trace {
        gate: Vec::with_capacity(total),
        frequency: Vec::with_capacity(total),
    };
    for t in 0..total {
        let clock = edges.iter().any(|&e| t >= e && t < e + PULSE);
        seq.set_input("clock", if clock { 1.0 } else { 0.0 })
            .unwrap();
        seq.process(1);
        trace.gate.push(seq.get_output("gate").unwrap());
        trace.frequency.push(seq.get_output("frequency").unwrap());
    }
    trace
}

/// Edges every `D` from 0 through `last` inclusive.
pub fn steady(last: usize) -> Vec<usize> {
    (0..=last / D).map(|k| k * D).collect()
}

/// The samples where `gate` rises past 0.5 (the first sample counts when it
/// starts high).
pub fn rises(gate: &[f32]) -> Vec<usize> {
    (0..gate.len())
        .filter(|&i| gate[i] > 0.5 && (i == 0 || gate[i - 1] <= 0.5))
        .collect()
}

/// Asserts the gate is high (`high`) or low over every sample of `range`.
pub fn assert_level(gate: &[f32], range: Range<usize>, high: bool, what: &str) {
    if let Some(i) = range.clone().find(|&i| (gate[i] > 0.5) != high) {
        let level = if high { "high" } else { "low" };
        panic!(
            "{what}: the gate should be {level} over {range:?} but is not at sample {i} \
             (rises at {:?})",
            rises(gate)
        );
    }
}

/// Asserts the bridged gate holds through `last + 2 * d + 1` and is low
/// from `last + 2 * d + 2` until `until`.
pub fn assert_release(gate: &[f32], last: usize, d: usize, until: usize, what: &str) {
    assert_level(gate, last + d..last + 2 * d + 2, true, what);
    assert_level(gate, last + 2 * d + 2..until, false, what);
}
