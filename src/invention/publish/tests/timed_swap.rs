//! A swap timed on a beat neither allocates, frees nor waits for a lock on
//! the audio thread as it lands, with every control-side lock held, and
//! lands on the sample its clock reaches the beat.

use super::migrated::block_with_locks_held;
use super::request_beats::{outcomes_of, render_counted, Twin};
use super::Rig;
use crate::control_request::{BeatSpec, Outcome, Timeline};
use crate::invention::publish::{EditState, EditTime};

/// A tempo with fractional samples per beat at 48 kHz.
const BPM: f64 = 97.0;

const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 97.0 } },
        { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
        { "id": "fx", "type": "filter", "config": { "cutoff": 400.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "fx", "to_port": "audio" },
        { "from": "fx", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

#[test]
fn a_swap_on_the_next_bar_lands_without_allocating_or_waiting() {
    let mut rig = Rig::new(DOCUMENT);
    // Into the first bar, so the next downbeat is beat 4.
    render_counted(&mut rig, 10);
    let lands = Twin::new(BPM).until(|clock| clock.position() >= 4.0) as usize;
    let bar = BeatSpec::Grid {
        every: 4.0,
        offset: 0.0,
        repeat: false,
    };
    // On the clock the filter counts on, the invention's only one.
    let time = EditTime::BeatOf {
        module: "fx",
        spec: bar,
    };
    let config = serde_json::json!({ "filter_type": "highpass" });
    let id = rig
        .live
        .swap_module_at(48_000, "fx", "filter", &config, true, time, None)
        .unwrap();
    assert_eq!(rig.live.timed_status(id).unwrap().state, EditState::Queued);

    let landing_block = lands / 64;
    render_counted(&mut rig, landing_block - 10);
    let counts = block_with_locks_held(&mut rig, &[]);
    assert_eq!(counts, Ok((0, 0)), "the landing block");
    render_counted(&mut rig, 2);

    let landed = Outcome::Applied { at: lands as u64 };
    assert_eq!(outcomes_of(&mut rig, id), [(id, landed)]);
    let adopted = rig.live.timed_status(id).unwrap();
    assert_eq!(adopted.state, EditState::Adopted);
    assert_eq!(adopted.built, ["fx"]);
}
