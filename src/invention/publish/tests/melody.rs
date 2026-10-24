//! A melody on a live graph: its writes, whole scales and whole weights
//! apply on the audio thread in the order they were made, from any number
//! of threads, without allocating, freeing or waiting on a lock; and a
//! write rewriting positions holds them, while the count rewrites none.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use super::migrated::{block_with_locks_held, read, schedule, write};
use super::request_beats::render_counted;
use super::requests::counted_block;
use super::*;
use crate::control_request::{
    BeatSpec, BeatTime, ControlIndex, Request, RequestValue, RtValue, Source, When,
};
use crate::ControlValue;

/// A melody playing an octave into an oscillator, its gate written by the
/// test, and a clock (2812.5 bpm at 48 kHz: a beat every 1024 samples) a
/// ramp can be timed on.
const MELODY: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "tune", "type": "melody", "config": {
            "degrees": [0, 2, 4, 5, 7, 9, 11, 12], "seed": 1 } },
        { "id": "clock", "type": "clock", "config": { "bpm": 2812.5 } },
        { "id": "osc", "type": "oscillator" },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "tune", "from_port": "frequency", "to": "osc", "to_port": "frequency" },
        { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

fn number(rig: &Rig, key: &str) -> f32 {
    match read(rig, "tune", key) {
        ControlValue::Number(number) => number,
        other => panic!("{key}: {other:?}"),
    }
}

/// What the positions in play hold.
fn playing(rig: &Rig, key: &str) -> Vec<f32> {
    let count = number(rig, "degree_count") as usize;
    (0..count)
        .map(|i| number(rig, &format!("{key}.{i}")))
        .collect()
}

/// One scheduler step: a block with its clock high, one with it low.
fn step(rig: &mut Rig) {
    for gate in [1.0, 0.0] {
        rig.live.write_input("sched", "clock", gate).unwrap();
        assert_eq!(counted_block(rig), (0, 0));
    }
}

#[test]
fn writes_and_whole_lists_apply_in_order_and_read_back() {
    let mut rig = Rig::new(MELODY);
    rig.render(1);
    write(&rig, "tune", "degree.1", 9.0.into());
    write(&rig, "tune", "degrees", "[0, 4, 7, 11]".into());
    write(&rig, "tune", "degree.2", 5.5.into());
    write(&rig, "tune", "note_weight.1", 3.0.into());
    write(&rig, "tune", "degree_count", 6.0.into());
    assert_eq!(number(&rig, "degree.1"), 2.0, "pending");
    assert_eq!(
        read(&rig, "tune", "degrees"),
        "[0,4,7,11]".into(),
        "as accepted"
    );

    assert_eq!(counted_block(&mut rig), (0, 0));
    // The whole scale replaced the earlier position write; the later one
    // stands, truncated as the melody holds it.
    assert_eq!(playing(&rig, "degree"), [0.0, 4.0, 5.0, 11.0, 0.0, 4.0]);
    assert_eq!(playing(&rig, "note_weight"), [1.0, 3.0, 1.0, 1.0, 1.0, 1.0]);
    assert_eq!(read(&rig, "tune", "degrees"), "[0,4,7,11]".into());
    let listed = {
        let surfaces = rig.live.control_surfaces.lock().unwrap();
        surfaces["tune"].controls().len()
    };
    assert_eq!(listed, 5 + 2 * 6, "the positions in play");
}

/// Every position, hidden ones included.
fn every_degree(rig: &Rig) -> Vec<f32> {
    (0..12)
        .map(|i| number(rig, &format!("degree.{i}")))
        .collect()
}

/// Every weight, hidden ones included.
fn every_weight(rig: &Rig) -> Vec<f32> {
    (0..12)
        .map(|i| number(rig, &format!("note_weight.{i}")))
        .collect()
}

/// A melody whose scale is negative markers, rendered: no writer below
/// writes a negative degree, so a marker read back is a replaced value.
fn marked() -> Rig {
    let mut rig = Rig::new(MELODY);
    let markers: Vec<String> = (0..12).map(|i| format!("{}", -50 - i)).collect();
    write(
        &rig,
        "tune",
        "degrees",
        format!("[{}]", markers.join(", ")).into(),
    );
    rig.render(1);
    rig
}

#[test]
fn concurrent_writers_never_lose_a_write_or_bring_back_a_replaced_value() {
    const ROUNDS: usize = 300;
    let mut rig = marked();
    let surface = rig.live.control_surfaces.lock().unwrap()["tune"].clone();
    // The writes as acknowledged, in the order made: each is logged as it
    // is accepted, under one lock the writers share (the audio thread never
    // takes it). Refused while the queue is full, a write is retried.
    let made = Mutex::new(Vec::new());
    let send = |key: &str, value: ControlValue| loop {
        let mut made = made.lock().unwrap();
        if surface.set_control(key, value.clone()).is_ok() {
            made.push((key.to_string(), value));
            return;
        }
        drop(made);
        std::thread::yield_now();
    };
    let scale = |i: usize| (0..4).map(|d| (i % 20 + d) as f32).collect::<Vec<_>>();
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let whole = scope.spawn(|| {
            for i in 0..ROUNDS {
                // Sets the count to 4 and every degree; keeps written weights.
                let degrees = scale(i).iter().map(f32::to_string).collect::<Vec<_>>();
                send("degrees", format!("[{}]", degrees.join(", ")).into());
                send("note_weight.2", ((i % 10) as f32).into());
            }
        });
        let positions = scope.spawn(|| {
            for i in 0..ROUNDS {
                send("note_weight.0", ((i % 7) as f32).into());
                send("note_weight.1", ((i % 9) as f32).into());
                // Past the scale: hidden, or shown by the count's growth.
                send(
                    &format!("degree.{}", 5 + i % 6),
                    ((30 + i % 50) as f32).into(),
                );
                let _ = surface.get_control("degree.1");
            }
        });
        let count = scope.spawn(|| {
            for i in 0..ROUNDS {
                send("degree_count", ((4 + i % 9) as f32).into());
            }
        });
        scope.spawn(|| {
            whole.join().unwrap();
            positions.join().unwrap();
            count.join().unwrap();
            done.store(true, Ordering::Release);
        });
        let mut replaced = false;
        while !done.load(Ordering::Acquire) {
            assert_eq!(counted_block(&mut rig), (0, 0));
            let degrees = every_degree(&rig);
            // Only a whole scale writes the first position.
            replaced |= degrees[0] >= 0.0;
            assert!(
                !replaced || degrees.iter().all(|degree| *degree >= 0.0),
                "a replaced degree came back: {degrees:?}"
            );
        }
    });
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert!(every_degree(&rig).iter().all(|degree| *degree >= 0.0));

    // Exactly what the acknowledged writes leave, made one at a time: every
    // write survives unless a later one rewrote it, and nothing it rewrote
    // comes back.
    let made = made.into_inner().unwrap();
    assert_eq!(made.len(), 6 * ROUNDS, "every write acknowledged once");
    let mut replayed = marked();
    let replay = replayed.live.control_surfaces.lock().unwrap()["tune"].clone();
    for (key, value) in made {
        while replay.set_control(&key, value.clone()).is_err() {
            assert_eq!(counted_block(&mut replayed), (0, 0));
        }
    }
    assert_eq!(counted_block(&mut replayed), (0, 0));
    assert_eq!(every_degree(&rig), every_degree(&replayed));
    assert_eq!(every_weight(&rig), every_weight(&replayed));
    assert_eq!(
        number(&rig, "degree_count"),
        number(&replayed, "degree_count")
    );
}

/// A whole scale, a count and a position written in each order, landing
/// in one block: they apply in that order, so no write made after the
/// scale is lost and nothing it replaced comes back.
#[test]
fn a_whole_scale_count_growth_and_position_write_apply_in_the_order_made() {
    let scale = || ("degrees", ControlValue::from("[20, 21, 22]"));
    let grow = || ("degree_count", ControlValue::from(6.0));
    let position = || ("degree.4", ControlValue::from(40.0));
    // (order, what positions 0 to 5 then hold, the count)
    let cases = [
        (
            [scale(), grow(), position()],
            [20.0, 21.0, 22.0, 20.0, 40.0, 22.0],
            6.0,
        ),
        (
            [scale(), position(), grow()],
            [20.0, 21.0, 22.0, 20.0, 40.0, 22.0],
            6.0,
        ),
        (
            [grow(), scale(), position()],
            [20.0, 21.0, 22.0, 20.0, 40.0, 22.0],
            3.0,
        ),
        (
            [position(), scale(), grow()],
            [20.0, 21.0, 22.0, 20.0, 21.0, 22.0],
            6.0,
        ),
        (
            [grow(), position(), scale()],
            [20.0, 21.0, 22.0, 20.0, 21.0, 22.0],
            3.0,
        ),
        (
            [position(), grow(), scale()],
            [20.0, 21.0, 22.0, 20.0, 21.0, 22.0],
            3.0,
        ),
    ];
    for (order, degrees, count) in cases {
        let mut rig = Rig::new(MELODY);
        // A degree written past a shrink, which the scale must erase.
        write(&rig, "tune", "degree_count", 2.0.into());
        write(&rig, "tune", "degree.5", 50.0.into());
        rig.render(1);
        let keys: Vec<&str> = order.iter().map(|(key, _)| *key).collect();
        for (key, value) in order {
            write(&rig, "tune", key, value);
        }
        assert_eq!(counted_block(&mut rig), (0, 0));
        assert_eq!(every_degree(&rig)[..6], degrees, "{keys:?}");
        assert_eq!(number(&rig, "degree_count"), count, "{keys:?}");
    }
}

#[test]
fn a_count_ramp_never_rewrites_a_held_position() {
    let mut rig = Rig::new(MELODY);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "tune", "control": "degree_count", "value": 4.0 },
            { "at_step": 1, "module": "tune", "control": "degree_count", "value": 8.0, "ramp_steps": 4 }
        ]}),
    );
    step(&mut rig);
    assert_eq!(number(&rig, "degree_count"), 4.0);
    // A user holds a hidden position and one in play.
    write(&rig, "tune", "degree.6", 3.0.into());
    write(&rig, "tune", "degree.1", 1.0.into());
    let mut counts = Vec::new();
    for _ in 0..5 {
        step(&mut rig);
        counts.push(number(&rig, "degree_count"));
        assert_eq!(number(&rig, "degree.6"), 3.0, "the hold wins");
        assert_eq!(number(&rig, "degree.1"), 1.0, "the hold wins");
    }
    // The count is not held by them: its ramp plays through.
    assert_eq!(counts.last(), Some(&8.0), "{counts:?}");
    assert!(
        counts.windows(2).all(|pair| pair[0] <= pair[1]),
        "{counts:?}"
    );
    assert_eq!(
        playing(&rig, "degree"),
        [0.0, 1.0, 4.0, 5.0, 7.0, 9.0, 3.0, 12.0]
    );
}

#[test]
fn a_whole_scale_holds_the_count_it_sets_off_a_count_ramp() {
    let mut rig = Rig::new(MELODY);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "tune", "control": "degree_count", "value": 2.0 },
            { "at_step": 1, "module": "tune", "control": "degree_count", "value": 8.0, "ramp_steps": 6 },
            { "at_step": 5, "module": "tune", "control": "degree_count", "value": 6.0 }
        ]}),
    );
    for _ in 0..3 {
        step(&mut rig);
    }
    let ramping = number(&rig, "degree_count");
    assert!(ramping > 2.0 && ramping < 8.0, "ramping: {ramping}");
    write(&rig, "tune", "degrees", "[0, 4, 7]".into());
    step(&mut rig);
    // The ramp is abandoned: the scale's count holds until the next entry.
    for _ in 0..2 {
        assert_eq!(number(&rig, "degree_count"), 3.0);
        step(&mut rig);
    }
    assert_eq!(number(&rig, "degree_count"), 6.0, "the next entry lands");
    assert_eq!(playing(&rig, "degree"), [0.0, 4.0, 7.0, 0.0, 4.0, 7.0]);
}

#[test]
fn a_whole_scale_holds_the_degrees_it_rewrites_off_their_ramps() {
    let mut rig = Rig::new(MELODY);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "tune", "control": "degree.1", "value": 12.0, "ramp_steps": 8 },
            { "at_step": 0, "module": "tune", "control": "degree.5", "value": 1.0, "ramp_steps": 8 },
            { "at_step": 7, "module": "tune", "control": "degree.1", "value": 3.0 }
        ]}),
    );
    step(&mut rig);
    step(&mut rig);
    let (one, five) = (number(&rig, "degree.1"), number(&rig, "degree.5"));
    assert!(one > 2.0 && five < 9.0, "ramping: {one}, {five}");
    write(&rig, "tune", "degrees", "[0, 4, 7]".into());
    // The rest of both ramps is dropped, in play and hidden.
    for _ in 0..4 {
        step(&mut rig);
        assert_eq!(number(&rig, "degree.1"), 4.0);
        assert_eq!(number(&rig, "degree.5"), 7.0);
    }
    step(&mut rig);
    step(&mut rig);
    assert_eq!(number(&rig, "degree.1"), 3.0, "the next entry lands");
    assert_eq!(number(&rig, "degree.5"), 7.0);
}

/// Submits `text` for payload control `key` at sample `at`, built as a
/// surface would build it.
fn submit_list_at(rig: &Rig, key: &str, text: &str, at: u64) {
    let publisher = rig.live.publisher().lock().unwrap();
    let (table, _) = rig.graph.modules["tune"].module().declared().unwrap();
    let index = table.resolve(key).unwrap();
    let codec = table.decl(index).unwrap().0.codec.unwrap();
    let (payload, _) = (codec.prepare)(&text.into()).unwrap();
    let target = publisher.control_target("tune", index).unwrap();
    let mut request = Request::new(target, RequestValue::Payload(payload));
    request.when = When::AtSample(at);
    rig.live.requests.submit(request).unwrap();
}

#[test]
fn automation_written_before_a_whole_scale_lands_before_it() {
    let mut rig = Rig::new(MELODY);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "tune", "control": "degree.1", "value": 9.0 }
        ]}),
    );
    // Clocked by the melody's gate, the scheduler shares a feedback loop
    // with it, played a sample at a time with the melody first: what the
    // scheduler writes at a sample, the melody takes at the next.
    rig.live
        .edit(|change| change.connect(edge("tune", "gate", "sched", "clock")))
        .unwrap();
    rig.render(1);
    let fired = rig.graph.current_sample;
    // The scale lands on the sample after the scheduler fires.
    submit_list_at(&rig, "degrees", "[0, 4, 7]", fired + 1);
    rig.live.write_input("tune", "gate", 1.0).unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(playing(&rig, "degree"), [0.0, 4.0, 7.0]);
}

/// Submits a user's glide of `key` to `value` over `beats` beats, from
/// the clock's next beat.
fn glide(rig: &Rig, key: &str, value: f32, beats: f32) {
    let publisher = rig.live.publisher().lock().unwrap();
    let (table, _) = rig.graph.modules["tune"].module().declared().unwrap();
    let target = publisher
        .control_target("tune", table.resolve(key).unwrap())
        .unwrap();
    let clock = publisher.control_target("clock", ControlIndex(0)).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(value)));
    request.when = When::Beat(BeatTime {
        clock: clock.module_idx as u32,
        spec: BeatSpec::Grid {
            every: 1.0,
            offset: 0.0,
            repeat: false,
        },
        ramp: Some(beats),
    });
    request.source = Source::User;
    rig.live.requests.submit(request).unwrap();
}

#[test]
fn whole_weights_hold_off_a_users_glide_on_a_weight_they_rewrite() {
    let mut rig = Rig::new(MELODY);
    rig.render(1);
    glide(&rig, "note_weight.2", 9.0, 4.0);
    render_counted(&mut rig, 24);
    let gliding = number(&rig, "note_weight.2");
    assert!(gliding > 1.0 && gliding < 9.0, "{gliding}");
    write(&rig, "tune", "note_weights", "[1, 1, 5]".into());
    render_counted(&mut rig, 80);
    assert_eq!(number(&rig, "note_weight.2"), 5.0, "the glide ended");
}

#[test]
fn ramps_and_writes_land_while_every_control_lock_is_held() {
    let mut rig = Rig::new(MELODY);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "tune", "control": "note_weight.0", "value": 5.0, "ramp_steps": 4 },
            { "at_step": 0, "module": "tune", "control": "degree.0", "value": 12.0, "ramp_steps": 4 },
            { "at_step": 0, "module": "tune", "control": "degree_count", "value": 12.0, "ramp_steps": 4 }
        ]}),
    );
    for (i, gate) in [1.0, 0.0, 1.0, 0.0, 1.0, 0.0].into_iter().enumerate() {
        write(&rig, "tune", "note_weight.3", (i as f32).into());
        if i == 2 {
            write(&rig, "tune", "note_weights", "[1, 2, 3]".into());
        }
        rig.live.write_input("sched", "clock", gate).unwrap();
        assert_eq!(block_with_locks_held(&mut rig, &[]), Ok((0, 0)));
    }
    // The whole weights rewrote the weight ramping, so they hold it.
    assert_eq!(number(&rig, "note_weight.0"), 1.0);
    assert!(number(&rig, "degree.0") > 0.0);
    assert!(number(&rig, "degree_count") > 8.0);
    assert_eq!(number(&rig, "note_weight.3"), 5.0);
}
