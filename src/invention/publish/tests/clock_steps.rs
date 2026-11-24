//! A sequencer clocked by a clock's gate output sizes its first gate from
//! the clock's tempo, which the graph hands it on the audio thread without
//! allocating, freeing or waiting, at the first block, after an edit
//! changes the routes and after a tempo change.

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

/// The clock alone, its beat_x4 gate unheard: a sequencer arrives later.
const CLOCK_ONLY: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 60.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": []
}"#;

#[test]
fn a_sequencer_a_timed_edit_lands_on_a_clock_edge_gets_a_full_first_gate() {
    use crate::invention::publish::timed::EditTime;
    let mut rig = Rig::new(CLOCK_ONLY);
    rig.render(8);
    let seq = rig.build(
        "seq",
        "step_sequencer",
        serde_json::json!({ "step_count": 2, "gate_length": 0.5,
            "pattern": [{ "note": 0 }, { "note": 7 }] }),
    );
    let mut change = rig.live.begin();
    change.upsert("seq", seq);
    change
        .connect(edge("clock", "beat_x4", "seq", "clock"))
        .unwrap();
    change.connect(edge("seq", "gate", "dac", "audio")).unwrap();
    // Mid-block, on the edge that starts the clock's second step.
    let landing = STEP as u64;
    (rig.live)
        .commit_when(
            change.prepare().unwrap(),
            EditTime::AtSample(landing),
            None,
            |_| {},
        )
        .unwrap();
    let before = rig.graph.current_sample as usize;
    let out = rig.render(4 * STEP / 64);
    // The edit fades the new route in, so the gate is heard a few
    // milliseconds after it rises: time it by where it falls.
    let first = out.iter().position(|&s| s > 0.5).unwrap();
    let first_end = before + first + gates(&out)[0];
    let landing = landing as usize;
    assert!(first_end.abs_diff(landing + STEP / 2) <= 2, "{first_end}");
    assert!(gates(&out)[1].abs_diff(STEP / 2) <= 2);
}

#[test]
fn a_tempo_written_on_an_edge_sizes_that_steps_gate_without_allocating() {
    use super::requests::submit;
    use crate::control_request::When;
    let mut rig = Rig::new(CLOCKED);
    // Up to the block holding the clock's third edge, which rises a sample
    // before 2 x STEP (the clock's first beat is a sample short).
    rig.render(2 * STEP / 64 - 1);
    // The clock's bpm, halved on that edge: a live write stages on the
    // clock, so it is taken before the sequencer measures the edge.
    submit(&rig, "clock", 0, 30.0, When::AtSample(2 * STEP as u64 - 1));
    assert_eq!(counted_block(&mut rig), (0, 0));
    let runs = gates(&rig.render(4 * STEP / 64));
    // The first run lost its first sample to the counted block.
    assert!(runs.len() >= 2, "{runs:?}");
    assert!(runs[0].abs_diff(STEP) <= 2, "{runs:?}");
    assert!(runs[1].abs_diff(STEP) <= 2, "{runs:?}");
}
