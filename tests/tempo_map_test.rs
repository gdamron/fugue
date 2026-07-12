//! Integration tests for score-scheduled tempo changes (FUG-189).
//!
//! A `control_scheduler` compiles a score tempo map into writes of the clock's
//! `bpm`, and the clock applies them phase-continuously: the beat grid keeps
//! its phase across the seam instead of jumping. These tests assert the clock's
//! beat-edge frames land exactly where the continuous beat count puts them —
//! the same frame-exactness discipline as the FUG-176 end gate and the
//! FUG-186 scheduled-control tests.

use fugue::{ControlValue, RenderEngine};

/// Renders `frames` frames one block at a time and returns the left channel,
/// which the inventions below drive directly with the clock's beat gate.
fn render_gate(engine: &mut RenderEngine, frames: usize) -> Vec<f32> {
    let block = engine.block_size();
    let mut buffer = vec![0.0f32; block * 2];
    let mut output = Vec::with_capacity(frames);
    while output.len() < frames {
        let n = (frames - output.len()).min(block);
        engine.render_interleaved(&mut buffer[..n * 2]).unwrap();
        for k in 0..n {
            output.push(buffer[k * 2]);
        }
    }
    output
}

/// Frame indices where the gate rises (low → high), treating the pre-roll as
/// low so an initial high frame counts as an edge.
fn rising_edges(gate: &[f32]) -> Vec<usize> {
    let mut edges = Vec::new();
    let mut prev = 0.0f32;
    for (frame, &value) in gate.iter().enumerate() {
        if prev <= 0.5 && value > 0.5 {
            edges.push(frame);
        }
        prev = value;
    }
    edges
}

/// A clock whose tempo map (spliced as a control_scheduler `tempo_map`) starts
/// at 22_500 BPM (128 samples/beat at 48 kHz) and speeds up to 28_800 BPM
/// (100 samples/beat) at step 3. The clock's beat gate is the output signal.
///
/// The seam is deliberately *not* phase-aligned to the new period: at the seam
/// frame the absolute sample count is 384, and 384 is not a multiple of 100.
/// A clock that derived phase from `sample_count % new_period` would drop the
/// beat and fire the next edge at frame 399 (sample 400); the phase-continuous
/// clock instead carries the beat phase across and fires ~100 frames later.
const TEMPO_MAP_INVENTION: &str = r#"{
    "version": "1.0.0",
    "title": "tempo-map-phase-continuity",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 22500.0 } },
        {
            "id": "tempo",
            "type": "control_scheduler",
            "config": {
                "tempo_map": [
                    { "at_step": 0, "bpm": 22500.0 },
                    { "at_step": 3, "bpm": 28800.0 }
                ]
            }
        },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "clock", "from_port": "gate", "to": "tempo", "to_port": "gate" },
        { "from": "clock", "from_port": "gate", "to": "dac", "to_port": "audio_left" }
    ]
}"#;

#[test]
fn tempo_map_beat_edges_are_phase_continuous_across_the_change() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(TEMPO_MAP_INVENTION).unwrap();
    engine.set_block_size(1);

    let gate = render_gate(&mut engine, 800);
    let edges = rising_edges(&gate);

    // Before the change, four beats at 128 samples/beat: frames 0, 127, 255,
    // 383 (the first period is one sample short because the clock
    // pre-increments its counter).
    assert_eq!(
        &edges[..4],
        &[0, 127, 255, 383],
        "pre-change beats must land on the 128-sample grid, got {edges:?}"
    );

    // The step-3 edge (frame 383) triggers the tempo change. The next beat must
    // arrive ~100 samples later — phase carried across the seam — not at frame
    // 399, where an absolute-modulo clock would misfire.
    let first_after = *edges
        .iter()
        .find(|&&frame| frame > 383)
        .expect("a beat must follow the tempo change");
    assert!(
        (478..=486).contains(&first_after),
        "first post-change beat should be ~100 samples after the seam, got {first_after}"
    );
    assert!(
        !edges.iter().any(|&frame| (384..470).contains(&frame)),
        "no spurious/early beat between the seam and the next beat: {edges:?}"
    );

    // The new tempo holds: subsequent beats are 100 samples apart.
    let post: Vec<usize> = edges
        .iter()
        .copied()
        .filter(|&f| f >= first_after)
        .collect();
    for pair in post.windows(2) {
        assert_eq!(
            pair[1] - pair[0],
            100,
            "post-change beats must be 100 samples apart, got {post:?}"
        );
    }
}

#[test]
fn tempo_map_render_is_byte_identical() {
    let render = || {
        let mut engine = RenderEngine::new(48_000);
        engine.load_json(TEMPO_MAP_INVENTION).unwrap();
        render_gate(&mut engine, 4_096)
    };
    let first: Vec<u32> = render().into_iter().map(f32::to_bits).collect();
    let second: Vec<u32> = render().into_iter().map(f32::to_bits).collect();
    assert_eq!(
        first, second,
        "two tempo-map renders must be byte-identical"
    );
}

#[test]
fn tempo_map_ramp_slows_smoothly_ritardando() {
    // A ramped tempo_map entry (ritardando) glides the clock from 120 to 40
    // BPM over 8 steps; the 16th-note grid must slow monotonically and
    // continuously, with no jump at the seam.
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "clock", "type": "clock", "config": { "bpm": 120.0, "gate_duration": 0.5 } },
                {
                    "id": "tempo",
                    "type": "control_scheduler",
                    "config": {
                        "tempo_map": [
                            { "at_step": 0, "bpm": 120.0 },
                            { "at_step": 4, "bpm": 40.0, "ramp": 8 }
                        ]
                    }
                },
                { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
            ],
            "connections": [
                { "from": "clock", "from_port": "gate_x4", "to": "tempo", "to_port": "gate" },
                { "from": "clock", "from_port": "gate_x4", "to": "dac", "to_port": "audio_left" }
            ]
        }"#,
        )
        .unwrap();
    engine.set_block_size(64);

    let gate = render_gate(&mut engine, 48_000 * 5);
    let edges = rising_edges(&gate);
    let spacings: Vec<i64> = edges
        .windows(2)
        .map(|w| w[1] as i64 - w[0] as i64)
        .collect();

    // Steps before the ramp are ♩=120 (6000 samples/16th).
    assert!(
        spacings[1..4].iter().all(|&s| (5999..=6001).contains(&s)),
        "pre-ramp grid should be ~6000 samples, got {spacings:?}"
    );
    // Through the ramp the spacing grows every step (monotonic slowing) and
    // never jumps backward — phase-continuous.
    let ramp = &spacings[4..12];
    for pair in ramp.windows(2) {
        assert!(
            pair[1] > pair[0],
            "ritardando must slow monotonically, got {ramp:?}"
        );
    }
    // Settles at ♩=40 (18000 samples/16th).
    assert!(
        spacings[13..16]
            .iter()
            .all(|&s| (17999..=18001).contains(&s)),
        "post-ramp grid should settle at ~18000 samples, got {spacings:?}"
    );
}

#[test]
fn tempo_map_scale_multiplies_the_written_bpm() {
    // bpm_scale is the invention's interpretation knob: a notated map (♩=60 →
    // ♩=30) with a x100 scale drives the clock at 6000 → 3000 BPM. (6000 BPM =
    // 480 samples/beat at 48 kHz, so step 2 lands at frame 959.)
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "clock", "type": "clock", "config": { "bpm": 6000.0 } },
                {
                    "id": "tempo",
                    "type": "control_scheduler",
                    "config": {
                        "tempo_map": [ { "at_step": 0, "bpm": 60.0 }, { "at_step": 2, "bpm": 30.0 } ],
                        "bpm_scale": 100.0
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "clock", "from_port": "gate", "to": "tempo", "to_port": "gate" }
            ]
        }"#,
        )
        .unwrap();
    engine.set_block_size(1);

    render_gate(&mut engine, 959);
    assert_eq!(
        engine.get_control("clock", "bpm").unwrap(),
        ControlValue::Number(6000.0),
        "before step 2 the clock runs at the scaled initial tempo (60 x 100)"
    );

    render_gate(&mut engine, 1); // frame 959: the step-2 edge
    assert_eq!(
        engine.get_control("clock", "bpm").unwrap(),
        ControlValue::Number(3000.0),
        "step 2 writes the scaled tempo (30 x 100)"
    );
}
