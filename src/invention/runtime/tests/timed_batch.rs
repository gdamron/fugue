//! Timed edit batches and reloads through the runtime's front door: the
//! whole edit lands on the next downbeat, its writes to modules it keeps
//! included, and nothing before; what cannot land whole is refused with
//! nothing queued; an authored write to a control it writes yields to it.

use super::*;
use crate::control_request::Timeline;
use crate::invention::manual_backend::{start_manual, Pump};
use crate::modules::Clock;
use crate::rpc::{EditWhen, RequestState, RpcErrorCode, StructuralEdit};
use crate::test_support::host_dial::HostDialFactory;

const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 480.0 } },
        { "id": "osc", "type": "oscillator", "config": { "waveform": "sawtooth", "frequency": 220.0 } },
        { "id": "fx", "type": "filter", "config": { "cutoff": 400.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "fx", "to_port": "audio" },
        { "from": "fx", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// Blocks of 64 past the second downbeat at 480 BPM.
const PAST_TWO_BARS: usize = 2 * 24_000 / 64 + 4;
/// Blocks into the first bar.
const INTO_THE_BAR: usize = 100;

fn start() -> (RunningInvention, Pump) {
    let invention = Invention::from_json(DOCUMENT).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    start_manual(runtime)
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

fn next_bar() -> EditWhen {
    EditWhen::NextBar {
        beats_per_bar: 4.0,
        clock: None,
    }
}

fn set(module_id: &str, key: &str, value: f32) -> StructuralEdit {
    StructuralEdit::SetControl {
        module_id: module_id.into(),
        key: key.into(),
        value: ControlValue::Number(value),
    }
}

fn add(id: &str, module_type: &str, config: serde_json::Value) -> StructuralEdit {
    StructuralEdit::AddModule {
        id: id.into(),
        module_type: module_type.into(),
        config,
    }
}

fn connect(from: &str, to: &str) -> StructuralEdit {
    StructuralEdit::Connect {
        from: from.into(),
        from_port: "audio".into(),
        to: to.into(),
        to_port: "audio".into(),
    }
}

/// Swaps the filter for a highpass one and retunes the oscillator.
fn swap_and_retune() -> Vec<StructuralEdit> {
    let highpass = serde_json::json!({ "filter_type": "highpass", "cutoff": 2000.0 });
    vec![
        StructuralEdit::RemoveModule { id: "fx".into() },
        add("fx", "filter", highpass),
        connect("osc", "fx"),
        connect("fx", "dac"),
        set("osc", "frequency", 330.0),
    ]
}

/// Where `running`'s output first differs from a twin's that never
/// edited, rendering both past the second downbeat from `INTO_THE_BAR`.
fn first_heard(audio: &Pump, twin: &Pump) -> Option<usize> {
    let (out, untouched) = (audio.render(PAST_TWO_BARS), twin.render(PAST_TWO_BARS));
    let differs = (0..out.len()).find(|&i| out[i] != untouched[i]);
    differs.map(|i| INTO_THE_BAR * 64 + i)
}

fn frequency_of(running: &RunningInvention) -> (f64, f64) {
    drop(running.document()); // caught up with what applied
    let state = running.state.lock().unwrap();
    let document = state.document.as_ref().unwrap();
    let osc = document
        .modules
        .iter()
        .find(|spec| spec.id == "osc")
        .unwrap();
    let stored = state.modules["osc"].config["frequency"].as_f64().unwrap();
    (osc.config["frequency"].as_f64().unwrap(), stored)
}

#[test]
fn an_edit_batch_on_the_next_downbeat_lands_whole_on_it() {
    let (mut running, audio) = start();
    let (_twin, twin) = start();
    assert_eq!(audio.render(INTO_THE_BAR), twin.render(INTO_THE_BAR));

    let queued = (running.apply_edits_at(&swap_and_retune(), &next_bar(), None)).unwrap();
    assert_eq!(queued.state, RequestState::Queued);
    assert_eq!(frequency_of(&running), (220.0, 220.0), "nothing yet");
    assert_eq!(first_heard(&audio, &twin), Some(downbeat()));

    let adopted = running.request_status(&queued.id).unwrap();
    assert_eq!(adopted.state, RequestState::Adopted);
    assert_eq!(adopted.built, ["fx"]);
    assert_eq!(frequency_of(&running), (330.0, 330.0));
    let frequency = running.get_control("osc", "frequency").unwrap();
    assert_eq!(frequency, ControlValue::Number(330.0));
}

#[test]
fn a_reload_on_the_next_downbeat_lands_whole_on_it() {
    let (mut running, audio) = start();
    let (_twin, twin) = start();
    assert_eq!(audio.render(INTO_THE_BAR), twin.render(INTO_THE_BAR));

    let reloaded = DOCUMENT
        .replace(
            r#""type": "filter", "config": { "cutoff": 400.0 }"#,
            r#""type": "vca", "config": {}"#,
        )
        .replace(r#""frequency": 220.0"#, r#""frequency": 330.0"#);
    let reloaded = Invention::from_json(&reloaded).unwrap();
    let queued = (running.reload_at(reloaded, &next_bar(), None)).unwrap();
    assert_eq!(queued.state, RequestState::Queued);
    assert_eq!(first_heard(&audio, &twin), Some(downbeat()));

    let adopted = running.request_status(&queued.id).unwrap();
    assert_eq!(adopted.state, RequestState::Adopted);
    assert_eq!(adopted.built, ["fx"]);
    assert_eq!(frequency_of(&running), (330.0, 330.0));
}

#[test]
fn what_cannot_land_whole_is_refused_with_nothing_queued() {
    let mut registry = crate::ModuleRegistry::default();
    registry.register(HostDialFactory);
    let document = DOCUMENT.replace(
        r#"{ "id": "dac""#,
        r#"{ "id": "ld", "type": "host_dial" }, { "id": "dac""#,
    );
    let invention = Invention::from_json(&document).unwrap();
    let (runtime, _) = InventionBuilder::with_registry(48_000, registry)
        .build(invention)
        .unwrap();
    let (mut running, audio) = start_manual(runtime);
    // An undeclared control of a module it keeps, which cannot be timed.
    let host = vec![
        add("lfo", "lfo", serde_json::json!({})),
        set("ld", "rate", 8.0),
    ];
    // No structure: a control write, for set_control to time.
    let controls_only = vec![set("osc", "frequency", 330.0)];
    for edits in [host, controls_only] {
        let error = running
            .apply_edits_at(&edits, &next_bar(), None)
            .unwrap_err();
        assert_eq!(error.code, RpcErrorCode::ControlError, "{error:?}");
    }
    assert!(running.take_request_changes().is_empty());
    audio.render(PAST_TWO_BARS);
    assert_eq!(frequency_of(&running), (220.0, 220.0));
    assert!(!running.state.lock().unwrap().modules.contains_key("lfo"));
}

/// The batch writes the oscillator's frequency as it lands: an authored
/// write to it meanwhile is kept over the batch's own, which still lands
/// whole otherwise and says what it kept, so the write is what plays and
/// what the document records.
#[test]
fn an_authored_write_to_a_control_the_batch_writes_is_kept_over_it() {
    let (mut running, audio) = start();
    audio.render(INTO_THE_BAR);
    let queued = (running.apply_edits_at(&swap_and_retune(), &next_bar(), None)).unwrap();
    running.take_request_changes();
    running
        .set_control("osc", "frequency", ControlValue::Number(275.0))
        .unwrap();
    let kept = "kept authored osc.frequency: the edit lands without writing it";
    let changed = running.take_request_changes();
    let reasons: Vec<_> = changed
        .iter()
        .map(|status| status.reason.as_deref())
        .collect();
    assert_eq!(reasons, [Some(kept)]);
    let status = running.request_status(&queued.id).unwrap();
    assert_eq!(
        (status.state, status.reason.as_deref()),
        (RequestState::Queued, Some(kept))
    );

    audio.render(PAST_TWO_BARS);
    let playing = running.get_control("osc", "frequency").unwrap();
    assert_eq!(playing, ControlValue::Number(275.0));
    assert_eq!(frequency_of(&running), (275.0, 275.0));
    let status = running.request_status(&queued.id).unwrap();
    assert_eq!(status.state, RequestState::Adopted);
    assert_eq!(status.built, ["fx"], "the swap landed");
    let landed = "kept authored osc.frequency: the edit landed without writing it";
    assert_eq!(status.reason.as_deref(), Some(landed));
}

/// An authored write to a control the batch leaves alone is kept, in the
/// document too, as the batch lands.
#[test]
fn an_authored_write_to_a_control_the_batch_leaves_alone_is_kept() {
    let (mut running, audio) = start();
    audio.render(INTO_THE_BAR);
    let queued = (running.apply_edits_at(&swap_and_retune(), &next_bar(), None)).unwrap();
    let saw = running.get_control("osc", "waveform").unwrap();
    running
        .set_control("osc", "waveform", ControlValue::String("sine".into()))
        .unwrap();
    assert_eq!(state_of(&running, &queued.id), RequestState::Queued);
    audio.render(PAST_TWO_BARS);
    assert_eq!(state_of(&running, &queued.id), RequestState::Adopted);
    assert_eq!(frequency_of(&running), (330.0, 330.0));
    let waveform = running.get_control("osc", "waveform").unwrap();
    assert_ne!(waveform, saw);
    let state = running.state.lock().unwrap();
    let document = state.document.as_ref().unwrap();
    let osc = document
        .modules
        .iter()
        .find(|spec| spec.id == "osc")
        .unwrap();
    assert_eq!(osc.config["waveform"], "sine", "kept: {}", osc.config);
}

fn state_of(running: &RunningInvention, id: &str) -> RequestState {
    running.request_status(id).unwrap().state
}
