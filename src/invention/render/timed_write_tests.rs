//! Timed control writes on an offline render land where the daemon's do:
//! the same piece, tempo and write render sample for sample alike, on a
//! sample, some samples on, or on a beat, and settle with the daemon's
//! fields. An authored one is recorded once it applies; one still waiting
//! as the piece is replaced is refused and never plays; one after another
//! request refuses its batch with nothing written; and the render takes
//! and applies them without allocating.

use std::sync::{Arc, Mutex};

use super::RenderEngine;
use crate::alloc_counter::allocator_events;
use crate::control_request::Timeline;
use crate::invention::manual_backend::{start_manual, Pump};
use crate::modules::dac::device_sample;
use crate::modules::Clock;
use crate::rpc::{
    ControlRequestSettled, ControlRequestState, ControlTiming, ControlWhen, ControlWriteReason,
};
use crate::{
    ControlValue, ControlWrite, ControlWriteIntent, Invention, InventionBuilder, RpcEventPayload,
    RunningInvention,
};

const RATE: u32 = 48_000;
const BLOCK: usize = 64;

/// A sine under a clock keeping bars at 480 BPM: a bar every 24,000
/// samples.
const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 480.0 } },
        { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// Blocks into the first bar, where each write is made.
const LEAD_IN: usize = 100;
/// Blocks rendered after it: past the second downbeat.
const AFTER: usize = 24_000 / BLOCK + 8;

#[derive(Default)]
struct Settled(Mutex<Vec<ControlRequestSettled>>);

impl crate::RpcEventSink for Settled {
    fn emit(&self, event: crate::RpcEvent) {
        if let RpcEventPayload::ControlRequestSettled(settled) = event.payload {
            self.0.lock().unwrap().push(settled);
        }
    }
}

fn daemon() -> (RunningInvention, Pump, Arc<Settled>) {
    let invention = Invention::from_json(DOCUMENT).unwrap();
    let (runtime, _) = InventionBuilder::new(RATE).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    let sink = Arc::new(Settled::default());
    running.set_event_sink(sink.clone());
    (running, pump, sink)
}

fn engine() -> RenderEngine {
    let mut engine = RenderEngine::new(RATE);
    engine.set_block_size(BLOCK);
    engine.load_json(DOCUMENT).unwrap();
    engine
}

/// `blocks` blocks of the daemon's output as the device receives it.
fn daemon_render(pump: &Pump, blocks: usize) -> Vec<f32> {
    let (left, right) = pump.render_stereo(blocks);
    let frames = left.iter().zip(&right);
    frames
        .flat_map(|(l, r)| [device_sample(*l), device_sample(*r)])
        .collect()
}

fn render(engine: &mut RenderEngine, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; 2 * BLOCK * blocks];
    engine.render_interleaved(&mut out).unwrap();
    out
}

fn timed(intent: ControlWriteIntent, when: ControlWhen) -> ControlWrite {
    ControlWrite {
        intent,
        timing: ControlTiming {
            when: Some(when),
            ttl_ms: None,
        },
        ..ControlWrite::new("osc", "frequency", ControlValue::Number(330.0))
    }
}

/// The sample the clock reaches beat `beat` on.
fn beat_sample(beat: f64) -> u64 {
    let mut clock = Clock::new(RATE, 480.0);
    (0..)
        .find(|_| {
            clock.tick();
            clock.position() >= beat
        })
        .unwrap()
}

/// Each kind of time, and the sample it lands on where that is plain:
/// one some beats on lands as the clock's position, read as the render
/// takes it, reaches it, which the daemon's landing shows.
fn whens() -> [(ControlWhen, Option<u64>); 5] {
    let now = (LEAD_IN * BLOCK) as u64;
    [
        (
            ControlWhen::AtSample { sample: now + 1000 },
            Some(now + 1000),
        ),
        (ControlWhen::AfterSamples { samples: 777 }, Some(now + 777)),
        (
            ControlWhen::AfterBeats {
                beats: 1.0,
                clock: None,
            },
            None,
        ),
        (
            ControlWhen::NextBeat {
                every: 2.0,
                offset: 0.0,
                clock: Some("clock".into()),
            },
            Some(beat_sample(2.0)),
        ),
        (
            ControlWhen::NextBar {
                beats_per_bar: Some(4.0),
                clock: None,
            },
            Some(beat_sample(4.0)),
        ),
    ]
}

#[test]
fn a_timed_write_lands_on_the_daemons_sample_and_settles_as_its_does() {
    for (when, lands) in whens() {
        let (running, pump, sink) = daemon();
        let mut engine = engine();
        let lead_in = render(&mut engine, LEAD_IN);
        assert_eq!(lead_in, daemon_render(&pump, LEAD_IN), "{when:?}");

        let write = timed(ControlWriteIntent::Perform, when.clone());
        let daemon_ack = running.write_controls(std::slice::from_ref(&write));
        let [daemon_ack] = &daemon_ack.controls[..] else {
            panic!("{when:?}: {daemon_ack:?}");
        };
        let [ack] = &engine.write_controls(&[write]).unwrap()[..] else {
            panic!("{when:?}: one entry");
        };
        assert_eq!(ack.state, Some(ControlRequestState::Pending), "{when:?}");
        assert_eq!(ack.request.is_some(), daemon_ack.request.is_some());
        assert_eq!(ack.landing, daemon_ack.landing, "{when:?}");
        assert_eq!(ack.value, None, "{when:?}: not read back before it lands");

        let output = render(&mut engine, AFTER);
        assert_eq!(output, daemon_render(&pump, AFTER), "{when:?}");
        let mut twin = self::engine();
        render(&mut twin, LEAD_IN);
        let unwritten = render(&mut twin, AFTER);
        let first = (0..output.len()).find(|&i| output[i] != unwritten[i]);
        let first = first.map(|i| (LEAD_IN * BLOCK + i / 2) as u64);

        running.announce_applied();
        let [settled] = &engine.take_settled_writes()[..] else {
            panic!("{when:?}: settled once");
        };
        let daemon_settled = sink.0.lock().unwrap();
        let [daemon_settled] = &daemon_settled[..] else {
            panic!("{when:?}: the daemon's settled once");
        };
        assert_eq!(settled.state, ControlRequestState::Applied, "{when:?}");
        assert_eq!(settled.landing, daemon_settled.landing, "{when:?}");
        let landed = settled.landing.sample.expect("applied on a sample");
        assert_eq!(lands.unwrap_or(landed), landed, "{when:?}");
        // The sine moves on the sample after the new frequency applies.
        assert_eq!(first, Some(landed + 1), "{when:?}");
        assert_eq!(Some(&settled.request), ack.request.as_ref());
        assert_eq!(
            engine.get_control("osc", "frequency").unwrap(),
            330.0.into()
        );
    }
}

#[test]
fn an_authored_timed_write_refuses_its_batch_with_nothing_written() {
    let mut engine = engine();
    let next_bar = ControlWhen::NextBar {
        beats_per_bar: Some(4.0),
        clock: None,
    };
    let performed = timed(ControlWriteIntent::Perform, next_bar.clone());
    let writes = [performed, timed(ControlWriteIntent::Author, next_bar)];
    let error = engine.write_controls(&writes).unwrap_err();
    let refusal = error.write.expect("names the write");
    assert_eq!(refusal.index, 1);
    assert_eq!(refusal.reason, ControlWriteReason::InvalidTiming);
    assert!(refusal.message.contains("perform"), "{}", refusal.message);
    assert!(refusal.message.contains("nothing was written"));
    render(&mut engine, AFTER);
    assert!(engine.take_settled_writes().is_empty());
    assert_eq!(
        engine.get_control("osc", "frequency").unwrap(),
        220.0.into()
    );
}

#[test]
fn an_authored_timed_write_to_an_event_control_is_timed_as_a_gesture() {
    let mut engine = engine();
    let reset = ControlWrite {
        intent: ControlWriteIntent::Author,
        timing: ControlTiming {
            when: Some(ControlWhen::AtSample { sample: 7000 }),
            ttl_ms: None,
        },
        ..ControlWrite::new("clock", "reset", ControlValue::Bool(true))
    };
    let [ack] = &engine.write_controls(&[reset]).unwrap()[..] else {
        panic!("one entry");
    };
    assert_eq!(ack.state, Some(ControlRequestState::Pending));
    render(&mut engine, AFTER);
    let [settled] = &engine.take_settled_writes()[..] else {
        panic!("settled once");
    };
    assert_eq!(settled.state, ControlRequestState::Applied);
}

#[test]
fn a_write_waiting_as_the_piece_is_replaced_is_refused_and_never_plays() {
    let mut engine = engine();
    let at = ControlWhen::AtSample { sample: 10_000 };
    engine
        .write_controls(&[timed(ControlWriteIntent::Perform, at)])
        .unwrap();
    engine.reset().unwrap();
    let [refused] = &engine.take_settled_writes()[..] else {
        panic!("settled once");
    };
    assert_eq!(refused.state, ControlRequestState::Refused);
    assert!(refused.reason.is_some());
    let mut fresh = self::engine();
    assert_eq!(render(&mut engine, 300), render(&mut fresh, 300));
    assert!(engine.take_settled_writes().is_empty());
}

#[test]
fn a_write_after_another_request_refuses_its_batch_with_nothing_written() {
    let mut engine = engine();
    // Whatever request it names, known or not.
    for request in ["1", "not-a-request"] {
        let after = ControlWhen::AfterRequest {
            request: request.into(),
            offset_samples: 0,
        };
        let untimed = ControlWrite::performed("osc", "frequency", ControlValue::Number(440.0));
        let writes = [untimed, timed(ControlWriteIntent::Perform, after)];
        let error = engine.write_controls(&writes).unwrap_err();
        let refusal = error.write.expect("names the write");
        assert_eq!(refusal.index, 1);
        assert_eq!(refusal.reason, ControlWriteReason::InvalidTiming);
        assert!(
            refusal.message.contains("after_request"),
            "{}",
            refusal.message
        );
        assert!(refusal.message.contains("nothing was written"));
    }
    assert_eq!(
        engine.get_control("osc", "frequency").unwrap(),
        220.0.into()
    );
    render(&mut engine, 10);
    assert!(engine.take_settled_writes().is_empty());
}

#[test]
fn a_write_before_one_after_another_request_is_refused_first() {
    let engine = engine();
    let unplaced = ControlWhen::NextBar {
        beats_per_bar: Some(4.0),
        clock: Some("no_such_clock".into()),
    };
    let after = ControlWhen::AfterRequest {
        request: "1".into(),
        offset_samples: 0,
    };
    let writes = [
        timed(ControlWriteIntent::Perform, unplaced),
        timed(ControlWriteIntent::Perform, after),
    ];
    let error = engine.write_controls(&writes).unwrap_err();
    let refusal = error.write.expect("names the write");
    assert_eq!(refusal.index, 0, "{}", refusal.message);
    assert_eq!(refusal.reason, ControlWriteReason::InvalidTiming);
}

#[test]
fn rendering_takes_and_applies_timed_writes_without_allocating() {
    let mut engine = engine();
    // The first block compiles the graph.
    render(&mut engine, 1);
    let writes: Vec<_> = (1..=8)
        .map(|n| ControlWhen::AtSample { sample: n * 700 })
        .chain([ControlWhen::NextBar {
            beats_per_bar: Some(4.0),
            clock: None,
        }])
        .map(|when| timed(ControlWriteIntent::Perform, when))
        .collect();
    engine.write_controls(&writes).unwrap();
    let mut out = vec![0.0f32; 2 * BLOCK * 400];
    let (_, allocs, frees) = allocator_events(|| engine.render_interleaved(&mut out).unwrap());
    assert_eq!((allocs, frees), (0, 0));
    let settled = engine.take_settled_writes();
    assert_eq!(settled.len(), writes.len());
    assert!(settled
        .iter()
        .all(|s| s.state == ControlRequestState::Applied));
}

fn edits(json: &str) -> Vec<crate::rpc::StructuralEdit> {
    serde_json::from_str(json).unwrap()
}

#[test]
fn a_write_waiting_through_an_edit_lands_on_its_module_and_sample() {
    let add = r#"[{"op":"add_module","id":"osc2","module_type":"oscillator","config":{"frequency":110.0}}]"#;
    let mut engine = engine();
    let mut twin = self::engine();
    render(&mut engine, LEAD_IN);
    render(&mut twin, LEAD_IN);
    let landing = (LEAD_IN * BLOCK) as u64 + 2000;
    let at = ControlWhen::AtSample { sample: landing };
    engine
        .write_controls(&[timed(ControlWriteIntent::Perform, at)])
        .unwrap();
    engine.apply_edits(&edits(add)).unwrap();
    twin.apply_edits(&edits(add)).unwrap();
    let output = render(&mut engine, 100);
    let unwritten = render(&mut twin, 100);
    let first = (0..output.len()).find(|&i| output[i] != unwritten[i]);
    let first = first.map(|i| (LEAD_IN * BLOCK + i / 2) as u64);
    assert_eq!(first, Some(landing + 1));
    let [settled] = &engine.take_settled_writes()[..] else {
        panic!("settled once");
    };
    assert_eq!(settled.state, ControlRequestState::Applied);
    assert_eq!(settled.landing.sample, Some(landing));
    assert_eq!(
        engine.get_control("osc", "frequency").unwrap(),
        330.0.into()
    );
    assert_eq!(
        engine.get_control("osc2", "frequency").unwrap(),
        110.0.into()
    );
}
