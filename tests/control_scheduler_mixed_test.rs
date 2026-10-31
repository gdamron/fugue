//! Two schedulers on one control, one stepping on a clock and one on an
//! LFO's rising edges: whichever entry comes later replaces the other's
//! ramp, whoever plays each schedule.

use fugue::{Invention, RenderEngine};

/// A clock at 120 bpm (a beat every 24000 samples) and an LFO rising at
/// 3 Hz (every 16000 samples, from sample 0) each step a scheduler on the
/// level of `trace`, which carries a constant 1.0 to the left channel.
fn render(on_clock: serde_json::Value, on_lfo: serde_json::Value) -> Vec<f32> {
    let doc = serde_json::json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
            { "id": "gate", "type": "lfo", "config": { "rate": 3.0, "waveform": "square" } },
            { "id": "dc", "type": "lfo", "config": { "rate": 0.01, "waveform": "square" } },
            { "id": "trace", "type": "vca", "config": { "level": 0.2 } },
            { "id": "by_clock", "type": "control_scheduler", "config": { "schedule": [on_clock] } },
            { "id": "by_lfo", "type": "control_scheduler", "config": { "schedule": [on_lfo] } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            { "from": "clock", "from_port": "beat", "to": "by_clock", "to_port": "clock" },
            { "from": "gate", "from_port": "unipolar", "to": "by_lfo", "to_port": "clock" },
            { "from": "dc", "from_port": "unipolar", "to": "trace", "to_port": "audio" },
            { "from": "trace", "from_port": "audio", "to": "dac", "to_port": "audio_left" }
        ]
    });
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_invention(Invention::from_json(&doc.to_string()).unwrap())
        .unwrap();
    let mut out = vec![0.0f32; 2 * 120_000];
    engine.render_interleaved(&mut out).unwrap();
    out.into_iter().step_by(2).collect()
}

fn entry(at_step: u64, value: f32, ramp_steps: u64) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "at_step": at_step, "module": "trace", "control": "level", "value": value
    });
    if ramp_steps > 0 {
        entry["ramp_steps"] = ramp_steps.into();
    }
    entry
}

#[test]
fn an_entry_on_edges_replaces_a_ramp_on_a_clock() {
    // The clock's ramp runs from step 0 over 8 beats; the LFO's step 2
    // (sample 32000) cuts the level.
    let left = render(entry(0, 1.0, 8), entry(2, 0.0, 0));
    assert!(left[30_000] > 0.2, "the ramp is under way");
    for at in [40_000, 72_000, 119_000] {
        assert_eq!(left[at], 0.0, "at {at}");
    }
}

#[test]
fn an_entry_on_a_clock_replaces_a_ramp_on_edges() {
    // The LFO's ramp runs from step 0 over 6 rises; the clock's step 1
    // (sample 24000) sets the level.
    let left = render(entry(1, 0.5, 0), entry(0, 1.0, 6));
    assert!(left[20_000] > 0.2, "the ramp is under way");
    for at in [30_000, 72_000, 119_000] {
        assert_eq!(left[at], 0.5, "at {at}");
    }
}
