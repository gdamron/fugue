//! Criterion 6: fast parameter sweeps make no steps at host block edges.

use super::rig::*;
use super::*;

/// Host block for the sweeps, and the sweep length (50 ms) in frames.
const HOST: usize = 256;
const SWEEP_FRAMES: f32 = 2400.0;
/// Blocks captured (about 128 ms), and the block at which the sweep starts.
const BLOCKS: usize = 24;
const START: usize = 2;
/// A block-edge step may be this multiple of the largest within-block step
/// of the two blocks around the edge.
const RATIO: f32 = 1.5;
/// Absolute slack: -120 dBFS.
const SLACK: f32 = 1e-6;

/// Blocks rendered between the strike and the first captured block. A damping
/// sweep starts right after the strike (at damping 0 the voice rings only
/// ~70 ms), so it is always made while the voice rings; the other keys wait
/// until the attack has passed.
fn settle_blocks(key: &str) -> usize {
    if key == "damping" {
        0
    } else {
        40
    }
}

/// Strikes a voice at 220 Hz with `key` at `from`, then moves `key` to `to`
/// within 50 ms: as ten per-block writes along a ramp, or (`jump`) as one
/// write of the end value. Captures `BLOCKS` blocks of all three outputs
/// from a block edge, and checks that every block-edge step is at most
/// `RATIO` times the largest step inside the neighbouring blocks.
fn sweep(position: usize, key: &str, from: f32, to: f32, jump: bool, model: &str) {
    let mut r = ready(position, 220.0, 0.9);
    r.set(key, from);
    r.render(seconds(0.3));
    r.strike();
    r.render_blocks(settle_blocks(key) * HOST, HOST);
    let mut out = Rendered::default();
    for block in 0..BLOCKS {
        if block >= START {
            let progress = if jump {
                1.0
            } else {
                (((block - START + 1) * HOST) as f32 / SWEEP_FRAMES).min(1.0)
            };
            if !jump || block == START {
                r.set(key, from + (to - from) * progress);
            }
        }
        let b = r.render_all(HOST, HOST);
        out.audio.extend(b.audio);
        out.odd.extend(b.odd);
        out.even.extend(b.even);
    }
    assert!(
        peak(&out.audio) > 1e-3,
        "{model}: silent during the sweep of {key}"
    );
    if key == "damping" {
        // The voice must still be ringing through the sweep, not just at the
        // strike: the first block with a damping write and the one after.
        let sweeping = &out.audio[START * HOST..(START + 2) * HOST];
        assert!(
            peak(sweeping) > 1e-2,
            "{model}: not ringing during the sweep of {key} {from}->{to}"
        );
    }
    for (name, x) in out.each() {
        for k in 1..BLOCKS {
            let edge = k * HOST;
            let step = (x[edge] - x[edge - 1]).abs();
            let within = (edge - HOST + 1..edge)
                .chain(edge + 1..(edge + HOST).min(x.len()))
                .map(|i| (x[i] - x[i - 1]).abs())
                .fold(0.0f32, f32::max);
            assert!(
                step <= RATIO * within + SLACK,
                "{model}: {key} {from}->{to} ({}): {name} steps {step} at block edge {k} \
                 (frame {edge}), within-block max {within}",
                if jump { "one write" } else { "ramp of writes" }
            );
        }
    }
}

fn sweep_every_model(keys: &[&str]) {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        for key in keys {
            for (from, to) in [(0.0f32, 1.0f32), (1.0, 0.0)] {
                for jump in [false, true] {
                    sweep(position, key, from, to, jump, model);
                }
            }
        }
    }
}

/// Criterion: "a fast `structure` sweep and a fast `brightness` sweep (0 to
/// 1 in 50 ms while ringing, host block 256) produce no steps at host block
/// edges": the largest step at a block-edge sample is at most 1.5x the
/// largest within-block step of the two blocks around it, on `audio`, `odd`
/// and `even`, for every model, in both directions, written as a per-block
/// ramp and as a single jump (which the module must glide).
#[test]
fn fast_structure_and_brightness_sweeps_have_no_block_edge_steps() {
    sweep_every_model(&["structure", "brightness"]);
}

/// Criterion (INTERFACE.md: "Each write glides (smoothed)"): the same for
/// `position` and `damping`.
/// Assumption: these two are held to the same bound as `structure` and
/// `brightness`. The `damping` sweeps start right after the strike so the
/// voice is ringing throughout (checked).
#[test]
fn fast_position_and_damping_sweeps_have_no_block_edge_steps() {
    sweep_every_model(&["position", "damping"]);
}

/// Criterion (INTERFACE.md: `level` is smoothed): a fast `level` sweep has
/// no block-edge steps either.
/// Assumption: the same bound applies to `level`.
#[test]
fn fast_level_sweep_has_no_block_edge_steps() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        for (from, to) in [(0.0f32, 1.0f32), (1.0, 0.0)] {
            sweep(position, "level", from, to, true, model);
        }
    }
}
