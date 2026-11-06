//! The self-excited models put no steps in their sound at engine block
//! edges while a macro sweeps.
//!
//! An engine reads its parameters once per engine block. One that applies
//! a new value all at once, instead of spreading it across the block,
//! turns its waveform harder on the first frame of each block than on any
//! other while the value moves. So the test sweeps each macro fast and
//! compares how sharply the sound turns at each frame position within the
//! block: a step makes one position stand out from the rest.

use super::glide_tests::{concrete, render};
use super::*;
use crate::dsp::sine;
use serde_json::json;

/// Whole engine blocks, so block edges stay at the same frame positions.
const HOST_BLOCK: usize = 40 * ENGINE_BLOCK;
/// How much sharper the sharpest frame position may turn than the median.
const MOST_EDGE_RATIO: f32 = 1.5;

/// The first model that excites itself; the models before it are
/// oscillators.
pub(super) fn first_excited_model() -> usize {
    engines::MODELS
        .iter()
        .position(|model| *model == "string")
        .expect("the string model")
}

/// The mean size of the second difference at each frame position within
/// the engine block, for `x` whose first frame is a block's first.
fn turn_by_position(x: &[f32]) -> [f32; ENGINE_BLOCK] {
    let mut sums = [0.0f32; ENGINE_BLOCK];
    let mut counts = [0usize; ENGINE_BLOCK];
    for (i, w) in x.windows(3).enumerate() {
        let position = (i + 1) % ENGINE_BLOCK;
        sums[position] += (w[2] - 2.0 * w[1] + w[0]).abs();
        counts[position] += 1;
    }
    std::array::from_fn(|i| sums[i] / counts[i].max(1) as f32)
}

/// The sharpest position's mean turn over the median position's.
fn edge_ratio(x: &[f32]) -> f32 {
    let mut turns = turn_by_position(x);
    let sharpest = turns.iter().copied().fold(0.0, f32::max);
    turns.sort_by(f32::total_cmp);
    let median = turns[ENGINE_BLOCK / 2];
    if median > 0.0 {
        sharpest / median
    } else if sharpest > 0.0 {
        f32::INFINITY
    } else {
        1.0
    }
}

#[test]
fn a_step_at_every_block_edge_stands_out() {
    let smooth: Vec<f32> = (0..4800).map(|i| sine(i as f32 * 0.003)).collect();
    assert!(edge_ratio(&smooth) < 1.2, "{}", edge_ratio(&smooth));
    // The same sine, with a value that steps at each block's first frame.
    let stepped: Vec<f32> = smooth
        .iter()
        .enumerate()
        .map(|(i, x)| x + 0.002 * (i / ENGINE_BLOCK) as f32 % 0.1)
        .collect();
    assert!(
        edge_ratio(&stepped) > 2.0 * MOST_EDGE_RATIO,
        "{}",
        edge_ratio(&stepped)
    );
}

#[test]
fn macro_sweeps_put_no_steps_at_block_edges() {
    const WRITES: usize = 10;
    for model in first_excited_model()..MODEL_COUNT {
        for key in ["harmonics", "timbre", "morph"] {
            for (from, to) in [(0.0f32, 1.0f32), (1.0, 0.0)] {
                for frequency in [55.0, 220.0] {
                    let mut module = concrete(json!({
                        "model": engines::MODELS[model], "frequency": frequency, key: from,
                    }));
                    render(&mut module, 24 * HOST_BLOCK, HOST_BLOCK);
                    // A write every host block, each gliding into the next:
                    // the whole range in 100 ms.
                    let mut swept = Vec::new();
                    for write in 1..=WRITES {
                        let value = from + (to - from) * write as f32 / WRITES as f32;
                        module.set_control(key, value).unwrap();
                        swept.extend(render(&mut module, HOST_BLOCK, HOST_BLOCK));
                    }
                    let ratio = edge_ratio(&swept);
                    assert!(
                        ratio <= MOST_EDGE_RATIO,
                        "{}: {key} {from} to {to} at {frequency} Hz turns {ratio} times harder at one block position",
                        engines::MODELS[model]
                    );
                }
            }
        }
    }
}
