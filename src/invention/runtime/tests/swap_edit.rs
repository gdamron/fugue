//! A swap through an edit batch sounds as `swap_module` does: the same
//! samples after it, immediate or on the next downbeat, and the same
//! document. A voice the swap does not touch plays on without a gap.

use super::*;
use crate::control_request::Timeline;
use crate::invention::manual_backend::{start_manual, Pump};
use crate::modules::Clock;
use crate::rpc::{EditWhen, RequestState, StructuralEdit};

/// A saw through a filter on the right channel, its cutoff swept by an
/// LFO, beside a sustained sine voice on the left; a clock keeps bars.
const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 480.0 } },
        { "id": "lfo", "type": "lfo", "config": { "rate": 3.0 } },
        { "id": "osc", "type": "oscillator", "config": { "waveform": "sawtooth", "frequency": 220.0 } },
        { "id": "fx", "type": "filter", "name": "Lead filter", "config": { "cutoff": 900.0 } },
        { "id": "voice", "type": "oscillator", "config": { "waveform": "sine", "frequency": 330.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "fx", "to_port": "audio" },
        { "from": "lfo", "from_port": "bipolar", "to": "fx", "to_port": "cutoff_mod" },
        { "from": "fx", "from_port": "audio", "to": "dac", "to_port": "audio_right" },
        { "from": "voice", "from_port": "audio", "to": "dac", "to_port": "audio_left" }
    ],
    "layout": { "modules": { "fx": { "x": 120, "y": 40 } } }
}"#;

/// Blocks of 64 into the first bar at 480 BPM.
const INTO_THE_BAR: usize = 100;
/// Blocks of 64 past the second downbeat from `INTO_THE_BAR`.
const PAST_TWO_BARS: usize = 2 * 24_000 / 64 + 4;

/// The swaps compared: two new types that drop the LFO's wire, and the
/// same type with a new config, which keeps every wire.
fn swaps() -> [(&'static str, serde_json::Value); 3] {
    [
        ("vca", serde_json::json!({})),
        (
            "filter",
            serde_json::json!({ "filter_type": "highpass", "cutoff": 2000.0 }),
        ),
        ("drive", serde_json::json!({})),
    ]
}

fn start() -> (RunningInvention, Pump) {
    let invention = Invention::from_json(DOCUMENT).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    start_manual(runtime)
}

fn swap_edit(module_type: &str, config: &serde_json::Value) -> StructuralEdit {
    StructuralEdit::SwapModule {
        id: "fx".into(),
        module_type: module_type.into(),
        config: config.clone(),
    }
}

fn next_bar() -> EditWhen {
    EditWhen::NextBar {
        beats_per_bar: Some(4.0),
        clock: None,
    }
}

/// The sample the clock reaches beat 4 on: the second bar's downbeat.
fn downbeat() -> usize {
    let mut clock = Clock::new(48_000, 480.0);
    (0..)
        .find(|_| {
            clock.tick();
            clock.position() >= 4.0
        })
        .unwrap()
}

#[test]
fn a_swap_in_a_batch_renders_sample_identical_to_swap_module() {
    for (module_type, config) in swaps() {
        let (mut batch, batch_audio) = start();
        let (tool, tool_audio) = start();
        assert_eq!(batch_audio.render_stereo(37), tool_audio.render_stereo(37));

        (batch.apply_edits(&[swap_edit(module_type, &config)])).expect("the batch commits");
        (tool.swap_module("fx", module_type, &config, true)).expect("the swap applies");
        let (left, right) = batch_audio.render_stereo(200);
        assert_eq!(
            (left, right.clone()),
            tool_audio.render_stereo(200),
            "{module_type}"
        );
        assert!(
            right.iter().any(|sample| *sample != 0.0),
            "{module_type} sounds"
        );
        assert_eq!(batch.document(), tool.document(), "{module_type}");
    }
}

#[test]
fn a_swap_on_the_next_downbeat_renders_sample_identical_to_a_timed_swap_module() {
    for (module_type, config) in swaps() {
        let (mut batch, batch_audio) = start();
        let (tool, tool_audio) = start();
        let (_twin, twin_audio) = start();
        let lead_in = batch_audio.render_stereo(INTO_THE_BAR);
        assert_eq!(lead_in, tool_audio.render_stereo(INTO_THE_BAR));
        assert_eq!(lead_in, twin_audio.render_stereo(INTO_THE_BAR));

        let edits = [swap_edit(module_type, &config)];
        let queued = (batch.apply_edits_at(&edits, &next_bar(), None)).unwrap();
        let timed =
            (tool.swap_module_at("fx", module_type, &config, true, &next_bar(), None)).unwrap();
        assert_eq!(
            (queued.state, timed.state),
            (RequestState::Queued, RequestState::Queued)
        );

        let (left, right) = batch_audio.render_stereo(PAST_TWO_BARS);
        assert_eq!(
            (left.clone(), right.clone()),
            tool_audio.render_stereo(PAST_TWO_BARS)
        );
        let (twin_left, twin_right) = twin_audio.render_stereo(PAST_TWO_BARS);
        let first = (0..right.len()).find(|&i| right[i] != twin_right[i]);
        assert_eq!(
            first.map(|i| INTO_THE_BAR * 64 + i),
            Some(downbeat()),
            "{module_type}"
        );
        assert_eq!(left, twin_left, "{module_type}: the untouched voice");

        let adopted = batch.request_status(&queued.id).unwrap();
        assert_eq!(adopted.state, RequestState::Adopted, "{module_type}");
        assert_eq!(batch.document(), tool.document(), "{module_type}");
    }
}

#[test]
fn a_voice_the_swap_does_not_touch_plays_on_without_a_gap() {
    let (mut running, audio) = start();
    let (_twin, twin) = start();
    let (left_before, _) = audio.render_stereo(50);
    assert_eq!(left_before, twin.render_stereo(50).0);

    let edits = [
        swap_edit("vca", &serde_json::json!({})),
        StructuralEdit::SetControl {
            module_id: "osc".into(),
            key: "frequency".into(),
            value: ControlValue::Number(110.0),
        },
    ];
    running.apply_edits(&edits).expect("the batch commits");
    let (left, right) = audio.render_stereo(200);
    let (twin_left, twin_right) = twin.render_stereo(200);
    // The sustained voice is sample for sample the one that never edited,
    // so its phase ran on through the publication; the swapped side moved.
    assert_eq!(left, twin_left);
    assert!(left.iter().any(|sample| sample.abs() > 0.5));
    assert_ne!(right, twin_right);
}

#[test]
fn a_timed_batch_cannot_swap_to_or_from_a_code_or_agent_module() {
    for module_type in ["code", "agent"] {
        let (mut running, audio) = start();
        let (_twin, twin) = start();
        let edits = [swap_edit(module_type, &serde_json::json!({}))];
        let error = (running.apply_edits_at(&edits, &next_bar(), None)).unwrap_err();
        assert!(error.message.contains("code or agent"), "{error:?}");
        assert!(!running.live.publisher().lock().unwrap().timed_waiting());
        assert_eq!(
            audio.render_stereo(PAST_TWO_BARS),
            twin.render_stereo(PAST_TWO_BARS)
        );
    }
    // Nor swap one out.
    let scripted = DOCUMENT.replace(
        r#"{ "id": "dac""#,
        r#"{ "id": "script", "type": "code", "config": { "script": "function init() {}" } },
        { "id": "dac""#,
    );
    let invention = Invention::from_json(&scripted).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let (mut running, _audio) = start_manual(runtime);
    let edits = [StructuralEdit::SwapModule {
        id: "script".into(),
        module_type: "vca".into(),
        config: serde_json::json!({}),
    }];
    let error = (running.apply_edits_at(&edits, &next_bar(), None)).unwrap_err();
    assert!(error.message.contains("code or agent"), "{error:?}");
    assert!(!running.live.publisher().lock().unwrap().timed_waiting());
}
