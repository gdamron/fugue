//! A graph with no clock: a zero-length block takes up what was submitted
//! and applies what is due now, advancing nothing, and a run on a
//! [`NullBackend`] settles every change that way before the call making it
//! returns.

use std::thread;

use serde_json::json;

use super::declared::dial_rig;
use super::requests::outcomes;
use super::*;
use crate::control_request::{ControlIndex, Outcome, Request, RequestValue, RtValue, When};
use crate::invention::publish::PendingWrite;
use crate::invention::runtime::RunningInvention;
use crate::modules::NullBackend;
use crate::test_support::dial::LEVEL;
use crate::ControlValue;

/// Queues `value` for the dial's level at `when`, as a front door would.
fn submit(rig: &Rig, value: f32, when: When) -> crate::control_request::RequestId {
    let mut publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("dial", LEVEL).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(value)));
    request.when = when;
    let id = rig.live.requests.submit(request).unwrap();
    publisher.note_written();
    id
}

fn level(rig: &Rig) -> RtValue {
    let module = rig.graph.modules["dial"].module();
    module.declared().unwrap().1.load(LEVEL).unwrap()
}

/// A zero-length block, which must not touch the allocator.
fn settle(rig: &mut Rig) {
    let ((), allocs, frees) = allocator_events(|| rig.graph.process_block(&mut [], &mut []));
    assert_eq!((allocs, frees), (0, 0));
}

#[test]
fn a_zero_length_block_applies_what_is_due_now_and_advances_nothing() {
    let mut rig = dial_rig();
    let now = rig.graph.current_sample;
    let later = submit(&rig, 0.75, When::AtSample(now + 1));
    let at_once = submit(&rig, 0.5, When::Now);

    settle(&mut rig);
    assert_eq!(level(&rig), RtValue::F32(0.5));
    assert_eq!(rig.graph.current_sample, now, "no time passed");
    assert_eq!(rig.graph.transport.rendered(), now);
    assert_eq!(
        outcomes(&mut rig),
        [(at_once, Outcome::Applied { at: now })]
    );

    // The timed request waits for its sample, which comes with the next
    // real block: it applies there, exactly on time.
    settle(&mut rig);
    assert_eq!(level(&rig), RtValue::F32(0.5));
    let out = rig.render(1);
    assert_eq!(out[0], 0.5);
    assert!(out[1..].iter().all(|v| *v == 0.75), "{out:?}");
    assert_eq!(
        outcomes(&mut rig),
        [(later, Outcome::Applied { at: now + 1 })]
    );
}

#[test]
fn a_zero_length_block_installs_a_publication_and_maps_requests_across_it() {
    let mut rig = dial_rig();
    let applied = rig.live.publisher().lock().unwrap().applied();
    let id = submit(&rig, 0.5, When::Now);
    // Published after the request, so the request is resolved against the
    // generation this one retires and is mapped across it.
    rig.publish_unreclaimed(|change| change.disconnect(edge("dial", "out", "dac", "audio")));

    settle(&mut rig);
    assert_eq!(rig.live.publisher().lock().unwrap().applied(), applied + 1);
    assert_eq!(level(&rig), RtValue::F32(0.5));
    let now = rig.graph.current_sample;
    assert_eq!(outcomes(&mut rig), [(id, Outcome::Applied { at: now })]);
}

/// The drain takes no lock a control thread holds: a zero-length block
/// finishes while another thread holds the publisher and the pending log.
#[test]
fn a_zero_length_block_never_waits_for_a_control_thread() {
    let mut rig = dial_rig();
    submit(&rig, 0.5, When::Now);
    let live = rig.live.clone();
    let graph = &mut rig.graph;
    let publisher = live.publisher().lock().unwrap();
    let pending = live.pending.lock().unwrap();
    thread::scope(|scope| {
        scope.spawn(|| graph.process_block(&mut [], &mut []));
    });
    drop((pending, publisher));
    assert_eq!(level(&rig), RtValue::F32(0.5));
}

const OSCILLATOR: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc", "type": "oscillator", "config": { "frequency": 440.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [{ "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }]
}"#;

fn start(json: &str) -> RunningInvention {
    let document = Invention::from_json(json).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(document).unwrap();
    runtime
        .start_with_backend(NullBackend::new(SAMPLE_RATE))
        .unwrap()
}

fn frequency(running: &RunningInvention, id: &str) -> ControlValue {
    running.get_control(id, "frequency").unwrap()
}

/// What the full snapshot reports `id`'s frequency as.
fn snapshot_frequency(running: &RunningInvention, id: &str) -> Option<ControlValue> {
    let snapshot = running.full_snapshot();
    let module = snapshot.modules.iter().find(|m| m.info.id == id)?;
    let control = module.controls.iter().find(|c| c.meta.key == "frequency")?;
    control.value.clone()
}

#[test]
fn a_write_on_a_null_backend_reads_back_at_once() {
    let running = start(OSCILLATOR);
    running
        .set_control("osc", "frequency", ControlValue::Number(220.0))
        .unwrap();
    assert_eq!(frequency(&running, "osc"), ControlValue::Number(220.0));
    assert_eq!(
        snapshot_frequency(&running, "osc"),
        Some(ControlValue::Number(220.0))
    );
    assert_eq!(running.live.pending_writes(), []);
}

/// Queues `value` for `key` of `id` at `when` as a timed front door will:
/// recorded as pending, then settled.
fn submit_timed(running: &RunningInvention, id: &str, key: &str, value: f32, when: When) {
    let surface = running.control_surfaces.lock().unwrap()[id].clone();
    let index: ControlIndex = surface.declaration(key).unwrap().index;
    let live = &running.live;
    {
        let mut publisher = live.publisher().lock().unwrap();
        let target = publisher.control_target(id, index).unwrap();
        let mut request = Request::new(target, RequestValue::Value(RtValue::F32(value)));
        request.when = when;
        let mut pending = live.pending.lock().unwrap();
        let request = live.requests.submit(request).unwrap();
        publisher.note_written();
        let write = PendingWrite {
            module_id: id.to_string(),
            key: key.to_string(),
            value: ControlValue::Number(value.into()),
        };
        pending.submitted(request, write);
    }
    live.settle();
}

#[test]
fn a_timed_write_on_a_null_backend_stays_pending() {
    let running = start(OSCILLATOR);
    let soon = std::time::Instant::now() + Duration::from_secs(1);
    submit_timed(&running, "osc", "frequency", 330.0, When::AfterSamples(1));
    submit_timed(&running, "osc", "frequency", 660.0, When::AtTime(soon));
    assert_eq!(frequency(&running, "osc"), ControlValue::Number(440.0));
    let pending: Vec<_> = running
        .live
        .pending_writes()
        .into_iter()
        .map(|write| write.value)
        .collect();
    assert_eq!(pending, [330.0.into(), 660.0.into()]);

    // A write timed now is not held up behind them.
    running
        .set_control("osc", "frequency", ControlValue::Number(220.0))
        .unwrap();
    assert_eq!(frequency(&running, "osc"), ControlValue::Number(220.0));
    assert_eq!(running.live.pending_writes().len(), 2);
}

#[test]
fn structural_edits_on_a_null_backend_take_writes_at_once() {
    let mut running = start(OSCILLATOR);
    running
        .add_module("osc2", "oscillator", &json!({ "frequency": 550.0 }))
        .unwrap();
    running.connect("osc2", "audio", "dac", "audio").unwrap();
    running
        .set_control("osc2", "frequency", ControlValue::Number(275.0))
        .unwrap();
    assert_eq!(frequency(&running, "osc2"), ControlValue::Number(275.0));

    // A swapped module takes writes through its new surface at once.
    running
        .swap_module("osc2", "oscillator", &json!({ "frequency": 110.0 }), true)
        .unwrap();
    running
        .set_control("osc2", "frequency", ControlValue::Number(120.0))
        .unwrap();
    assert_eq!(frequency(&running, "osc2"), ControlValue::Number(120.0));

    // A reload's config changes land as control writes, read back at once.
    let report = running
        .reload(Invention::from_json(&OSCILLATOR.replace("440.0", "880.0")).unwrap())
        .unwrap();
    assert_eq!(report.controls_updated, ["osc.frequency"]);
    assert_eq!(report.removed, ["osc2"]);
    assert_eq!(frequency(&running, "osc"), ControlValue::Number(880.0));
    assert!(running.get_control("osc2", "frequency").is_err());
}

#[test]
fn a_kept_snapshot_is_refused_once_a_null_backend_stops() {
    let running = start(OSCILLATOR);
    let kept = running.snapshot();
    running.stop();
    let refused = kept
        .set_control("osc", "frequency", ControlValue::Number(220.0))
        .unwrap_err();
    assert!(refused.to_string().contains("stopped"), "{refused}");
}
