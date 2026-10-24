//! Timed swaps through the runtime's front door, in the wire's terms: an
//! effect swapped on the next downbeat lands on it and is reported queued,
//! landed and adopted with the module it built; one whose ttl passes first
//! expires, one another edit overtakes is cancelled with the reason, and a
//! time that can never come is refused with nothing queued.

use super::*;
use crate::control_request::Timeline;
use crate::invention::manual_backend::{start_manual, Pump};
use crate::modules::Clock;
use crate::rpc::{EditWhen, RequestState, RequestStatus};
use crate::LoggedBeat;

/// 480 BPM at 48 kHz: a beat every 6000 samples, a bar of four every 24000.
const BPM: f64 = 480.0;
const BAR: usize = 24_000;
/// Blocks of 64 past the second downbeat.
const PAST_TWO_BARS: usize = 2 * BAR / 64 + 4;

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

fn start() -> (RunningInvention, Pump) {
    let invention = Invention::from_json(DOCUMENT).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    start_manual(runtime)
}

/// The sample the clock reaches beat `beat` on.
fn downbeat(beat: f64) -> usize {
    let mut clock = Clock::new(48_000, BPM);
    let mut sample = 0;
    loop {
        clock.tick();
        if clock.position() >= beat {
            return sample;
        }
        sample += 1;
    }
}

fn next_bar() -> EditWhen {
    EditWhen::NextBar {
        beats_per_bar: 4.0,
        clock: None,
    }
}

/// Swaps `fx` for a highpass filter, at `when` once the ttl allows.
fn swap_fx(
    running: &RunningInvention,
    when: EditWhen,
    ttl_ms: Option<u64>,
) -> Result<RequestStatus, GraphCommandError> {
    let config = serde_json::json!({ "filter_type": "highpass", "cutoff": 2000.0 });
    running.swap_module_at("fx", "filter", &config, true, &when, ttl_ms)
}

fn state_of(running: &RunningInvention, id: &str) -> RequestState {
    running.request_status(id).unwrap().state
}

#[test]
fn an_effect_swapped_on_the_next_downbeat_lands_on_it() {
    let (running, audio) = start();
    let (_twin, twin_audio) = start();
    // Into the first bar: the next downbeat is the second bar's.
    assert_eq!(audio.render(100), twin_audio.render(100));

    let queued = swap_fx(&running, next_bar(), None).unwrap();
    assert_eq!(queued.state, RequestState::Queued);
    let beat = LoggedBeat::Grid {
        clock: "clock".to_string(),
        every: 4.0,
        offset: 0.0,
        repeat: false,
    };
    assert_eq!(queued.landing.beat, Some(beat), "on the fx's clock");
    assert!(running.request_handles(&queued.id).is_none(), "not yet");

    let out = audio.render(PAST_TWO_BARS);
    let untouched = twin_audio.render(PAST_TWO_BARS);
    let heard = 100 * 64 + (0..out.len()).find(|&i| out[i] != untouched[i]).unwrap();
    let downbeat = downbeat(4.0);
    assert_eq!(heard, downbeat, "the swap is heard from the downbeat");

    let changes = running.take_request_changes();
    let adopted = changes.last().unwrap();
    assert_eq!(adopted.id, queued.id);
    assert_eq!(adopted.state, RequestState::Adopted);
    assert_eq!(adopted.landing.sample, Some(downbeat as u64));
    assert_eq!(adopted.landing.due, None, "on time");
    assert_eq!(adopted.built, ["fx"]);
    assert!(adopted.removed.is_empty());
    assert!(running.take_request_changes().is_empty());

    assert!(running.request_handles(&queued.id).is_some());
    audio.render(1);
    assert_eq!(
        running.get_control("fx", "cutoff").unwrap(),
        2000.0.into(),
        "the new filter's"
    );
    assert!(running.request_handles(&queued.id).is_none(), "taken once");
    let state = running.state.lock().unwrap();
    assert_eq!(state.modules["fx"].config["filter_type"], "highpass");
}

#[test]
fn a_swap_whose_ttl_passes_before_the_downbeat_expires() {
    let (running, audio) = start();
    // Past the first downbeat, which a bar queued at sample 0 lands on.
    audio.render(100);
    let queued = swap_fx(&running, next_bar(), Some(10)).unwrap();
    audio.render(PAST_TWO_BARS);
    let expired = running.request_status(&queued.id).unwrap();
    assert_eq!(expired.state, RequestState::Expired);
    assert!(expired.reason.unwrap().contains("ttl"));
    let state = running.state.lock().unwrap();
    assert_eq!(state.modules["fx"].config["cutoff"], 400.0, "kept");
}

#[test]
fn a_swap_another_edit_overtakes_is_cancelled_with_the_reason() {
    let (running, audio) = start();
    let queued = swap_fx(&running, next_bar(), None).unwrap();
    running
        .add_module("lfo", "lfo", &serde_json::json!({}))
        .unwrap();
    audio.render(PAST_TWO_BARS);
    let cancelled = running.request_status(&queued.id).unwrap();
    assert_eq!(cancelled.state, RequestState::Cancelled);
    let reason = cancelled.reason.unwrap();
    assert!(
        reason.starts_with("cancelled: another structural edit"),
        "{reason}"
    );
    assert_eq!(state_of(&running, &queued.id), RequestState::Cancelled);
}

#[test]
fn a_swap_waiting_as_the_invention_stops_is_cancelled_and_never_lands() {
    let (running, audio) = start();
    let queued = swap_fx(&running, next_bar(), None).unwrap();
    running.settle_requests();
    let cancelled = running.request_status(&queued.id).unwrap();
    assert_eq!(cancelled.state, RequestState::Cancelled);
    assert!(cancelled.reason.unwrap().contains("stopping"));
    audio.render(PAST_TWO_BARS);
    assert_eq!(state_of(&running, &queued.id), RequestState::Cancelled);
    let state = running.state.lock().unwrap();
    assert_eq!(state.modules["fx"].config["cutoff"], 400.0, "kept");
}

#[test]
fn a_swap_landed_as_the_invention_stops_is_adopted_into_the_document() {
    let (running, audio) = start();
    audio.render(100);
    let at = EditWhen::AtSample { sample: 6400 };
    let queued = swap_fx(&running, at, None).unwrap();
    audio.render(PAST_TWO_BARS);
    running.settle_requests();
    assert_eq!(state_of(&running, &queued.id), RequestState::Adopted);
    let document = running.document().unwrap();
    let fx = document.modules.iter().find(|m| m.id == "fx").unwrap();
    assert_eq!(fx.config["cutoff"], 2000.0);
}

#[test]
fn a_time_that_can_never_come_is_refused_with_nothing_queued() {
    let (running, _audio) = start();
    let not_a_clock = EditWhen::AfterBeats {
        beats: 1.0,
        clock: Some("osc".to_string()),
    };
    let backwards = EditWhen::AfterBeats {
        beats: -1.0,
        clock: None,
    };
    let no_bar = EditWhen::NextBar {
        beats_per_bar: 0.0,
        clock: None,
    };
    for when in [not_a_clock, backwards, no_bar] {
        let error = swap_fx(&running, when, None).unwrap_err();
        assert!(
            matches!(error, GraphCommandError::ControlError(_)),
            "{error}"
        );
    }
    assert!(running.take_request_changes().is_empty());
    assert!(running.request_status("1").is_none());
    assert!(running.request_status("not an id").is_none());
}
