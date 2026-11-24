//! A sequencer clocked by a clock's gate output sizes its first gate from
//! the clock's tempo, which the graph hands it on the audio thread without
//! allocating, freeing or waiting, at the first block and after an edit
//! changes the routes.

use super::requests::counted_block;
use super::*;

/// A clock at 60 bpm stepping a sequencer every quarter beat (12,000
/// samples), its gate heard on the dac.
const CLOCKED: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 60.0 } },
        { "id": "seq", "type": "step_sequencer", "config": {
            "step_count": 2, "gate_length": 0.5,
            "pattern": [{ "note": 0 }, { "note": 7 }] } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "clock", "from_port": "beat_x4", "to": "seq", "to_port": "clock" },
        { "from": "seq", "from_port": "gate", "to": "dac", "to_port": "audio" }
    ]
}"#;

const STEP: usize = 12_000;

/// The lengths of the runs of gate-high samples in `out`.
fn gates(out: &[f32]) -> Vec<usize> {
    let mut runs = Vec::new();
    let mut run = 0;
    for &sample in out {
        if sample > 0.5 {
            run += 1;
        } else if run > 0 {
            runs.push(run);
            run = 0;
        }
    }
    runs
}

#[test]
fn the_first_gate_is_as_long_as_the_later_ones_and_costs_nothing() {
    assert_eq!(counted_block(&mut Rig::new(CLOCKED)), (0, 0));
    let out = Rig::new(CLOCKED).render(4 * STEP / 64);
    let runs = gates(&out);
    assert!(runs.len() >= 3, "{runs:?}");
    for run in &runs[..3] {
        assert!(run.abs_diff(STEP / 2) <= 2, "{runs:?}");
    }
    assert!(runs[0].abs_diff(runs[2]) <= 2, "{runs:?}");
}

#[test]
fn an_edit_that_moves_the_clock_route_is_told_without_allocating() {
    let mut rig = Rig::new(CLOCKED);
    rig.render(1);
    rig.publish_unreclaimed(|change| {
        change.disconnect(edge("clock", "beat_x4", "seq", "clock"));
        change
            .connect(edge("clock", "beat_x2", "seq", "clock"))
            .unwrap();
    });
    assert_eq!(counted_block(&mut rig), (0, 0));
}
