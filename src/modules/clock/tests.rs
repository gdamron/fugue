//! The clock's `bpm` output.

use super::*;
use crate::control_request::{apply_declared, RtValue};
use crate::MAX_BLOCK;

fn bpm_output(clock: &Clock, frames: usize) -> Vec<f32> {
    let index = clock.output_port_index("bpm").expect("a bpm output");
    clock.output_block(index)[..frames].to_vec()
}

#[test]
fn the_bpm_output_carries_the_tempo_on_every_sample() {
    let mut clock = Clock::new(48_000, 97.5);
    clock.process(MAX_BLOCK);
    assert!(bpm_output(&clock, MAX_BLOCK).iter().all(|&bpm| bpm == 97.5));

    apply_declared(&mut clock, BPM, RtValue::F32(140.0)).unwrap();
    clock.process(64);
    assert!(bpm_output(&clock, 64).iter().all(|&bpm| bpm == 140.0));
    assert_eq!(clock.get_output("bpm").unwrap(), 140.0);
}

#[test]
fn the_bpm_output_carries_no_gates_and_leaves_the_gate_outputs_in_place() {
    let clock = Clock::new(48_000, 120.0);
    assert_eq!(
        clock.outputs(),
        &["beat", "beat_d4", "beat_d2", "beat_x2", "beat_x4", "beat_x8", "bpm"]
    );
    assert!(clock.gate_beats("bpm").is_none());
    assert_eq!(
        clock.gate_beats("beat_x4").map(|(_, beats)| beats),
        Some(0.25)
    );
    assert_eq!(
        clock.gate_beats("beat_x8").map(|(_, beats)| beats),
        Some(0.125)
    );
}

#[test]
fn beat_x8_at_90_bpm_rises_where_beat_x4_at_180_bpm_does() {
    // A clock at 90 bpm on `beat_x8` steps a 32nd-note grid exactly as one
    // at 180 bpm on `beat_x4` does: the same rising samples.
    let rises = |bpm: f64, port: &str| {
        let mut clock = Clock::new(44_100, bpm);
        let index = clock.output_port_index(port).unwrap();
        let mut rises = Vec::new();
        let mut last = 0.0;
        for block in 0..(441_000 / MAX_BLOCK) {
            clock.process(MAX_BLOCK);
            for (i, &gate) in clock.output_block(index)[..MAX_BLOCK].iter().enumerate() {
                if gate > 0.5 && last <= 0.5 {
                    rises.push(block * MAX_BLOCK + i);
                }
                last = gate;
            }
        }
        rises
    };
    let thirty_seconds = rises(90.0, "beat_x8");
    assert_eq!(thirty_seconds.len(), 120);
    assert_eq!(thirty_seconds, rises(180.0, "beat_x4"));
}

#[test]
fn adding_the_bpm_output_leaves_the_gates_unchanged() {
    // The gates a 120 bpm clock rises on at 48 kHz: every 24,000 samples
    // for `beat`, from sample 0 (the first output sample is one past it).
    let mut clock = Clock::new(48_000, 120.0);
    let beat = clock.output_port_index("beat").unwrap();
    let mut rises = Vec::new();
    let mut last = 0.0;
    for block in 0..(100_000 / MAX_BLOCK) {
        clock.process(MAX_BLOCK);
        for (i, &gate) in clock.output_block(beat)[..MAX_BLOCK].iter().enumerate() {
            if gate > 0.5 && last <= 0.5 {
                rises.push(block * MAX_BLOCK + i);
            }
            last = gate;
        }
    }
    assert_eq!(&rises[..4], &[0, 23_999, 47_999, 71_999]);
}
