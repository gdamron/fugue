//! Timeline context (B3): a request timed in beats that names no clock
//! counts on the clock driving its module through its connections. The
//! clock is resolved on the control thread before the request is submitted,
//! so a request keeps it through later edits, and the audio thread does
//! nothing new: every block here stays allocation- and free-free.

use super::request_beats::{blocks_for, render_counted, Twin};
use super::requests::outcomes;
use super::*;
use crate::control_request::{
    BeatSpec, BeatTime, ControlIndex, Outcome, Refusal, Request, RequestId, RequestValue, RtValue,
    Timeline, When,
};
use crate::invention::timelines::Context;

/// vca `level`, its one declared control.
const LEVEL: ControlIndex = ControlIndex(0);

/// Two clocks, `a` first in module order, each driving a sequencer; `osc`
/// is driven by neither.
const TWO_CLOCKS: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "a", "type": "clock", "config": { "bpm": 120.0 } },
        { "id": "b", "type": "clock", "config": { "bpm": 97.0 } },
        { "id": "seq_a", "type": "step_sequencer", "config": {} },
        { "id": "seq_b", "type": "step_sequencer", "config": {} },
        { "id": "voice", "type": "vca" },
        { "id": "osc", "type": "oscillator", "config": { "frequency": 110.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "a", "from_port": "beat", "to": "seq_a", "to_port": "clock" },
        { "from": "b", "from_port": "beat_d2", "to": "seq_b", "to_port": "clock" },
        { "from": "seq_b", "from_port": "gate", "to": "voice", "to_port": "level" },
        { "from": "voice", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// The position of module `id` in module order.
fn index(rig: &Rig, id: &str) -> usize {
    let publisher = rig.live.publisher().lock().unwrap();
    publisher.mirror().modules.get_index_of(id).unwrap()
}

/// Module `id`'s timeline context, as the publisher holds it.
fn timeline(rig: &Rig, id: &str) -> Context {
    let index = index(rig, id);
    rig.live.publisher().lock().unwrap().mirror().timelines[index]
}

fn connected(rig: &Rig, clock: &str, distance: u32) -> Context {
    let clock = index(rig, clock);
    Context::Connected { clock, distance }
}

fn fallback(rig: &Rig, clock: &str) -> Context {
    let clock = index(rig, clock);
    Context::Fallback { clock }
}

/// The clock an unnamed request on `id` resolves to, by id.
fn resolves_to(rig: &Rig, id: &str) -> Result<String, GraphCommandError> {
    let publisher = rig.live.publisher().lock().unwrap();
    let clock = publisher.beat_clock(id, None)?;
    Ok(publisher
        .mirror()
        .modules
        .get_index(clock as usize)
        .unwrap()
        .0
        .clone())
}

/// Submits `value` for `module_id`'s `level` at `spec`, naming no clock, as
/// a front door would: resolved and submitted under the publisher.
fn submit_unnamed(rig: &Rig, module_id: &str, value: f32, spec: BeatSpec) -> RequestId {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target(module_id, LEVEL).unwrap();
    let clock = publisher.beat_clock(module_id, None).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(value)));
    request.when = When::Beat(BeatTime { clock, spec });
    rig.live.requests.submit(request).unwrap()
}

/// The sample a fresh clock at `bpm` reaches `beats` beats past its
/// position at `start`.
fn beat_sample(bpm: f64, start: u64, beats: f64) -> u64 {
    let mut twin = Twin::new(bpm);
    twin.run_to(start);
    let goal = twin.clock.position().max(0.0) + beats;
    twin.until(|clock| clock.position() >= goal)
}

#[test]
fn each_sequencer_resolves_to_the_clock_driving_it() {
    let rig = Rig::new(TWO_CLOCKS);
    assert_eq!(timeline(&rig, "seq_a"), connected(&rig, "a", 1));
    assert_eq!(timeline(&rig, "seq_b"), connected(&rig, "b", 1));
    assert_eq!(resolves_to(&rig, "seq_a").unwrap(), "a");
    assert_eq!(resolves_to(&rig, "seq_b").unwrap(), "b");
    // Downstream of seq_b only, though `a` comes first.
    assert_eq!(timeline(&rig, "voice"), connected(&rig, "b", 2));
    assert_eq!(resolves_to(&rig, "voice").unwrap(), "b");
    // A clock is on its own timeline.
    assert_eq!(timeline(&rig, "b"), connected(&rig, "b", 0));
    // The dac hears `b` through the voice, and `osc` nothing.
    assert_eq!(timeline(&rig, "dac"), connected(&rig, "b", 3));
    // A named clock wins over the context.
    let publisher = rig.live.publisher().lock().unwrap();
    assert_eq!(publisher.beat_clock("voice", Some("a")).unwrap(), 0);
}

#[test]
fn a_module_no_clock_drives_falls_back_to_the_first_clock() {
    let rig = Rig::new(TWO_CLOCKS);
    assert_eq!(timeline(&rig, "osc"), fallback(&rig, "a"));
    assert_eq!(resolves_to(&rig, "osc").unwrap(), "a");
}

#[test]
fn with_no_clock_at_all_an_unnamed_request_is_refused() {
    let rig = Rig::new(BASE);
    assert_eq!(timeline(&rig, "osc1"), Context::None);
    assert!(matches!(
        resolves_to(&rig, "osc1"),
        Err(GraphCommandError::ControlError(_))
    ));
    assert!(matches!(
        resolves_to(&rig, "nope"),
        Err(GraphCommandError::UnknownModule(_))
    ));
}

#[test]
fn a_repatch_moves_the_module_but_not_what_already_waits() {
    let mut rig = Rig::new(TWO_CLOCKS);
    rig.render(3);
    let start = rig.graph.current_sample;
    let before = submit_unnamed(&rig, "voice", 0.5, BeatSpec::After(2.0));
    // Re-patch the voice from seq_b (clock b) to clock a.
    rig.live
        .edit(|change| {
            change.disconnect(edge("seq_b", "gate", "voice", "level"));
            change.connect(edge("a", "beat", "voice", "level"))
        })
        .unwrap();
    assert_eq!(timeline(&rig, "voice"), connected(&rig, "a", 1));
    let after = submit_unnamed(&rig, "voice", 0.25, BeatSpec::After(2.0));
    render_counted(&mut rig, blocks_for(97.0, 3.0));

    // The request made before keeps clock b; the one after counts on a.
    let on_b = beat_sample(97.0, start, 2.0);
    let on_a = beat_sample(120.0, start, 2.0);
    assert_ne!(on_a, on_b);
    let mut got = outcomes(&mut rig);
    got.sort_by_key(|(id, _)| *id);
    assert_eq!(
        got,
        [
            (before, Outcome::Applied { at: on_b }),
            (after, Outcome::Applied { at: on_a }),
        ]
    );
}

#[test]
fn a_waiting_request_whose_resolved_clock_goes_is_refused() {
    let mut rig = Rig::new(TWO_CLOCKS);
    rig.render(1);
    let id = submit_unnamed(&rig, "voice", 0.5, BeatSpec::After(4.0));
    rig.live
        .edit(|change| {
            change.remove("b");
            Ok(())
        })
        .unwrap();
    // The context moves on; the request does not follow it.
    assert_eq!(timeline(&rig, "voice"), fallback(&rig, "a"));
    render_counted(&mut rig, blocks_for(120.0, 5.0));
    assert_eq!(
        outcomes(&mut rig),
        [(id, Outcome::Refused(Refusal::TimelineGone))]
    );
}
