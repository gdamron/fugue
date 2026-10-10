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
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("dial", LEVEL).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(value)));
    request.when = when;
    // Queued before the publisher is released, so ahead of any later edit.
    rig.live.requests.submit(request).unwrap()
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

/// A clockless drain never leaves a request queued for want of room: one
/// that finds the store full of requests for samples that never come is
/// refused at once, rather than waiting behind them for ever.
#[test]
fn a_clockless_drain_refuses_what_a_full_store_cannot_take() {
    let mut rig = dial_rig();
    rig.graph.requests.as_mut().unwrap().clockless = true;
    let now = rig.graph.current_sample;
    for batch in 0..2 {
        for i in 0..publisher::REQUEST_QUEUE_CAPACITY {
            let at = now + 1 + (batch * publisher::REQUEST_QUEUE_CAPACITY + i) as u64;
            submit(&rig, 0.75, When::AtSample(at));
        }
        settle(&mut rig);
    }
    assert!(rig.graph.requests.as_ref().unwrap().pending.is_full());
    let id = submit(&rig, 0.5, When::Now);

    settle(&mut rig);
    assert_eq!(level(&rig), RtValue::F32(0.25));
    let refused = Outcome::Refused(crate::control_request::Refusal::PendingFull);
    assert_eq!(outcomes(&mut rig), [(id, refused)]);
}

/// A graph a [`NullBackend`] renders, linked to settle through it, and the
/// backend's raw render handle. No reclaimer thread runs.
fn null_linked(json: &str) -> (LiveGraph, NullBackend, crate::modules::dac::Settle) {
    let document = Invention::from_json(json).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(document).unwrap();
    let ports = Arc::new(Mutex::new(module_ports(&runtime.modules)));
    let mut graph = SignalGraph::new(
        runtime.modules,
        runtime.sinks,
        runtime.routing,
        MasterObservers::default(),
    );
    graph.recompile();
    let mut backend = NullBackend::new(SAMPLE_RATE);
    let render = backend.settle_handle();
    let live = LiveGraph::link(
        &mut graph,
        runtime.state,
        runtime.control_surfaces,
        ports,
        Arc::new(runtime.registry),
        Some(render.clone()),
    );
    let block = move |left: &mut [f32], right: &mut [f32]| graph.process_block(left, right);
    crate::AudioBackend::start(&mut backend, Box::new(block)).unwrap();
    (live, backend, render)
}

/// Settling frees what earlier blocks retired first, so a publication
/// waiting behind a full retire ring installs without another change.
#[test]
fn settling_installs_a_publication_a_full_retire_ring_held_back() {
    let (live, _backend, render) = null_linked(OSCILLATOR);
    let osc_to_dac = edge("osc", "audio", "dac", "audio");
    for n in 0..publisher::RETIRE_CAPACITY + 2 {
        let mut publisher = live.publisher().lock().unwrap();
        let mut change = live.change_on(&publisher);
        if n % 2 == 0 {
            change.disconnect(osc_to_dac.clone());
        } else {
            change.connect(osc_to_dac.clone()).unwrap();
        }
        publisher.publish(change.prepare().unwrap()).unwrap();
        drop(publisher);
        // A block that frees nothing first, as the audio thread renders.
        render.settle(|| {});
    }
    let published = live.generation();
    let applied = || live.publisher().lock().unwrap().applied();
    assert!(applied() < published, "the ring held the last one back");

    live.settle();
    assert_eq!(applied(), published);
}

/// A settle takes the publisher first, so it waits for a change in
/// progress and then takes up everything queued under it: a request queued
/// behind a publication not yet installed applies before the settle
/// returns.
#[test]
fn settling_waits_for_the_publisher() {
    let (live, _backend, _render) = null_linked(OSCILLATOR);
    let surface = live.control_surfaces.lock().unwrap()["osc"].clone();
    let index = surface.declaration("frequency").unwrap().index;
    let (settled, waiting) = std::sync::mpsc::channel();
    let live = &live;
    thread::scope(|scope| {
        // Published, not yet installed, with a request queued behind it.
        let mut publisher = live.publisher().lock().unwrap();
        let mut change = live.change_on(&publisher);
        change.disconnect(edge("osc", "audio", "dac", "audio"));
        publisher.publish(change.prepare().unwrap()).unwrap();
        let target = publisher.control_target("osc", index).unwrap();
        let request = Request::new(target, RequestValue::Value(RtValue::F32(220.0)));
        live.requests.submit(request).unwrap();
        scope.spawn(move || {
            live.settle();
            settled.send(()).unwrap();
        });
        let early = waiting.recv_timeout(Duration::from_millis(200));
        assert!(early.is_err(), "settled while the publisher was held");
    });
    assert_eq!(
        surface.get_control("frequency").unwrap(),
        ControlValue::Number(220.0)
    );
}

/// A block installs a bounded number of edits, so a settle renders until
/// every edit published before it has installed: a caller whose edit came
/// after a burst of others still finds it applied when its settle returns.
#[test]
fn a_settle_installs_every_edit_published_before_it() {
    let (live, _backend, _render) = null_linked(OSCILLATOR);
    let surface = live.control_surfaces.lock().unwrap()["osc"].clone();
    let index = surface.declaration("frequency").unwrap().index;
    let fm = edge("osc", "audio", "dac", "audio");
    {
        let mut publisher = live.publisher().lock().unwrap();
        for n in 0..=2 * crate::invention::graph::MAX_INSTALLS_PER_BLOCK {
            let mut change = live.change_on(&publisher);
            if n % 2 == 0 {
                change.disconnect(fm.clone());
            } else {
                change.connect(fm.clone()).unwrap();
            }
            publisher.publish(change.prepare().unwrap()).unwrap();
        }
        let target = publisher.control_target("osc", index).unwrap();
        let request = Request::new(target, RequestValue::Value(RtValue::F32(220.0)));
        live.requests.submit(request).unwrap();
    }
    live.settle();
    let publisher = live.publisher().lock().unwrap();
    assert_eq!(publisher.applied(), publisher.generation());
    drop(publisher);
    assert_eq!(
        surface.get_control("frequency").unwrap(),
        ControlValue::Number(220.0)
    );
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
        let publisher = live.publisher().lock().unwrap();
        let target = publisher.control_target(id, index).unwrap();
        let mut request = Request::new(target, RequestValue::Value(RtValue::F32(value)));
        request.when = when;
        let mut pending = live.pending.lock().unwrap();
        let request = live.requests.submit(request).unwrap();
        let write = PendingWrite {
            module_id: id.to_string(),
            key: key.to_string(),
            value: ControlValue::Number(value),
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

/// A settle whose render panics while requests apply (the drain is out of
/// the graph then) unwinds rather than hanging on the publisher it holds:
/// the drain dropped while unwinding neither closes the publisher nor
/// frees what is queued.
#[test]
fn a_settle_whose_render_panics_unwinds_rather_than_hangs() {
    fn panics(
        _: &mut SignalGraph,
        _: usize,
        _: crate::control_request::ControlIndex,
        _: RequestValue,
        _: &mut crate::payload::Retirer,
    ) -> Result<(), crate::control_request::Refusal> {
        panic!("a module panicked applying a request");
    }
    let document = Invention::from_json(OSCILLATOR).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(document).unwrap();
    let ports = Arc::new(Mutex::new(module_ports(&runtime.modules)));
    let mut graph = SignalGraph::new(
        runtime.modules,
        runtime.sinks,
        runtime.routing,
        MasterObservers::default(),
    );
    graph.recompile();
    graph.request_hook = Some(panics);
    let mut backend = NullBackend::new(SAMPLE_RATE);
    let live = LiveGraph::link(
        &mut graph,
        runtime.state,
        runtime.control_surfaces,
        ports,
        Arc::new(runtime.registry),
        Some(backend.settle_handle()),
    );
    let block = move |left: &mut [f32], right: &mut [f32]| graph.process_block(left, right);
    crate::AudioBackend::start(&mut backend, Box::new(block)).unwrap();
    let surface = live.control_surfaces.lock().unwrap()["osc"].clone();
    let index = surface.declaration("frequency").unwrap().index;
    {
        let publisher = live.publisher().lock().unwrap();
        let target = publisher.control_target("osc", index).unwrap();
        let request = Request::new(target, RequestValue::Value(RtValue::F32(220.0)));
        live.requests.submit(request).unwrap();
    }

    let settling = std::thread::spawn(move || live.settle());
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !settling.is_finished() {
        assert!(std::time::Instant::now() < deadline, "the settle hung");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        settling.join().is_err(),
        "the render's panic reaches the settle"
    );
}
