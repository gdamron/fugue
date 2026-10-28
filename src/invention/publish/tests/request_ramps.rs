//! Ramps in beats on a live graph: a ramp glides its control from its
//! beat and lands exactly on its end sample, a user's write holds it off
//! until automation's next event, and every block stays allocation-,
//! free- and lock-free.

use super::migrated::{block_with_locks_held, schedule};
use super::request_beats::{outcomes_of, render_counted};
use super::requests::outcomes;
use super::*;
use crate::control_request::{
    ramp_value, BeatSpec, BeatTime, Outcome, Refusal, Request, RequestId, RequestValue, RtValue,
    Source, When, RAMP_CAPACITY,
};
use crate::test_support::dial::{DialFactory, DIAL, LEVEL};

/// Samples per beat: 2812.5 bpm at 48 kHz.
const BEAT: usize = 1024;

/// A clock whose beat `n` falls on sample `n * BEAT`, and a dial (level
/// 0.25) alone into an unclipped dac: the output is the dial's level.
fn ramp_rig() -> Rig {
    let mut rig = Rig::new(
        r#"{
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 2812.5 } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": []
    }"#,
    );
    rig.registry.register(DialFactory);
    let dial = rig.build("dial", DIAL, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.connect(edge("dial", "out", "dac", "audio"))
        })
        .unwrap();
    rig
}

/// Submits a write of `level` to the dial from `source` at `spec` on the
/// clock, gliding over `ramp` beats when given.
fn submit_level(rig: &Rig, source: Source, level: f32, when: When, ramp: Option<f32>) -> RequestId {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("dial", LEVEL).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(level)));
    request.when = when;
    if let When::Beat(time) = &mut request.when {
        time.ramp = ramp;
    }
    request.source = source;
    rig.live.requests.submit(request).unwrap()
}

/// On the clock, at the next beat `n` with `n % every == offset`.
fn on_beat(rig: &Rig, every: f32, offset: f32) -> When {
    let publisher = rig.live.publisher().lock().unwrap();
    let clock = publisher.control_target("clock", LEVEL).unwrap();
    When::Beat(BeatTime {
        clock: clock.module_idx as u32,
        spec: BeatSpec::Grid {
            every,
            offset,
            repeat: false,
        },
        ramp: None,
    })
}

/// The sample beat 1 falls on, in a rig of its own.
fn beat_one() -> usize {
    let mut probe = ramp_rig();
    let id = submit_level(
        &probe,
        Source::Automation,
        0.5,
        on_beat(&probe, 4.0, 1.0),
        None,
    );
    render_counted(&mut probe, 32);
    let [(_, Outcome::Applied { at })] = outcomes_of(&mut probe, id)[..] else {
        panic!("beat 1 applies on time");
    };
    at as usize
}

#[test]
fn a_ramp_glides_from_its_beat_and_lands_exactly_on_its_end_sample() {
    let start = beat_one();
    let mut rig = ramp_rig();
    let when = on_beat(&rig, 4.0, 1.0);
    let id = submit_level(&rig, Source::Automation, 0.75, when, Some(2.0));
    let out = render_counted(&mut rig, 64);
    let applied = Outcome::Applied { at: start as u64 };
    assert_eq!(outcomes_of(&mut rig, id), [(id, applied)]);
    assert!(out[..start].iter().all(|&level| level == 0.25));
    // From beat 1 it holds, through each segment, the value for the
    // clock's position at the segment's start (its own start, then each
    // block start), and arrives exactly on beat 3.
    let end = start + 2 * BEAT;
    let mut level = 0.25;
    for (sample, &got) in out.iter().enumerate().take(end).skip(start) {
        if sample == start || sample % 64 == 0 {
            let progress = (sample - start) as f64 / (2 * BEAT) as f64;
            level = ramp_value(0.25, 0.75, progress);
        }
        assert_eq!(got, level, "{sample}");
    }
    assert!(out[end..].iter().all(|&level| level == 0.75));
    assert!(out[end - 1] < 0.75);
}

#[test]
fn a_users_write_holds_a_ramp_off_until_automations_next_event() {
    let mut rig = ramp_rig();
    let when = on_beat(&rig, 4.0, 1.0);
    submit_level(&rig, Source::Automation, 0.75, when, Some(2.0));
    // Halfway, at beat 2, a user's write takes the dial.
    let held = submit_level(&rig, Source::User, 0.1, When::AtSample(2048), None);
    // Automation's next event on it, at beat 3.5, takes it back.
    let when = on_beat(&rig, 4.0, 3.5);
    let next = submit_level(&rig, Source::Automation, 0.6, when, None);
    let out = render_counted(&mut rig, 64);
    let settled = outcomes(&mut rig);
    let at = |id| match settled.iter().find(|(got, _)| *got == id) {
        Some((_, Outcome::Applied { at })) => *at as usize,
        other => panic!("{other:?}"),
    };
    let (held_at, next_at) = (at(held), at(next));
    assert_eq!(held_at, 2048);
    assert!(out[1024..held_at].iter().all(|&level| level < 0.5));
    // The ramp is dropped: it neither moves the dial nor lands at beat 3.
    assert!(out[held_at..next_at].iter().all(|&level| level == 0.1));
    assert!(next_at > 3 * BEAT + BEAT / 4, "{next_at}");
    assert!(out[next_at..].iter().all(|&level| level == 0.6));
}

#[test]
fn automations_next_write_on_its_control_replaces_a_ramp() {
    let mut rig = ramp_rig();
    let when = on_beat(&rig, 4.0, 1.0);
    submit_level(&rig, Source::Automation, 0.75, when, Some(2.0));
    // Timed in samples, at its own tier: no hold drops the ramp, the
    // write itself ends it.
    submit_level(&rig, Source::Automation, 0.1, When::AtSample(2048), None);
    let out = render_counted(&mut rig, 64);
    assert!(out[2048..].iter().all(|&level| level == 0.1));
}

#[test]
fn a_schedulers_entry_on_its_control_replaces_a_ramp() {
    let mut rig = ramp_rig();
    let when = on_beat(&rig, 8.0, 1.0);
    submit_level(&rig, Source::Automation, 0.75, when, Some(3.0));
    // Its entry at step 2 is automation's next event on the dial, mid-ramp.
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 2, "module": "dial", "control": "level", "value": 0.1 }
        ] }),
    );
    rig.live
        .edit(|change| change.connect(edge("clock", "beat", "sched", "clock")))
        .unwrap();
    let out = render_counted(&mut rig, 128);
    let entry = out.iter().position(|&level| level == 0.1).unwrap();
    assert!((BEAT + BEAT / 2..3 * BEAT).contains(&entry), "{entry}");
    assert!(out[1024..entry].iter().any(|&level| level > 0.25));
    assert!(out[entry..].iter().all(|&level| level == 0.1));
}

#[test]
fn a_running_ramp_takes_no_lock_a_control_thread_holds() {
    let mut rig = ramp_rig();
    let when = on_beat(&rig, 4.0, 1.0);
    submit_level(&rig, Source::Automation, 0.75, when, Some(2.0));
    render_counted(&mut rig, 20);
    for _ in 0..4 {
        assert_eq!(block_with_locks_held(&mut rig, &[]), Ok((0, 0)));
    }
}

#[test]
fn a_ramp_without_room_is_refused() {
    let mut rig = ramp_rig();
    // Every ramp's room taken by ramps far off.
    for _ in 0..RAMP_CAPACITY {
        let when = on_beat(&rig, 64.0, 63.0);
        submit_level(&rig, Source::Automation, 0.5, when, Some(1.0));
    }
    render_counted(&mut rig, 1);
    outcomes(&mut rig);
    let when = on_beat(&rig, 4.0, 1.0);
    let id = submit_level(&rig, Source::Automation, 0.5, when, Some(1.0));
    render_counted(&mut rig, 1);
    let full = Outcome::Refused(Refusal::PendingFull);
    assert_eq!(outcomes_of(&mut rig, id), [(id, full)]);
}

#[test]
fn a_ramp_stopping_its_own_clock_still_reaches_its_end_beat() {
    use super::request_beats::BPM;
    let mut rig = ramp_rig();
    let when = on_beat(&rig, 4.0, 1.0);
    let id = {
        let publisher = rig.live.publisher().lock().unwrap();
        let target = publisher.control_target("clock", BPM).unwrap();
        let mut request = Request::new(target, RequestValue::Value(RtValue::F32(0.0)));
        request.when = when;
        if let When::Beat(time) = &mut request.when {
            time.ramp = Some(0.0001);
        }
        request.source = Source::Automation;
        drop(publisher);
        rig.live.requests.submit(request).unwrap()
    };
    render_counted(&mut rig, 64);
    let [(_, Outcome::Applied { .. })] = outcomes_of(&mut rig, id)[..] else {
        panic!("the ramp starts");
    };
    // Its last write, of 0 bpm, lands on the first sample past its end:
    // the clock stops there, not short of it.
    let clock = rig.graph.modules["clock"].module().timeline().unwrap();
    let stopped = clock.position();
    assert!(
        (1.0001..1.0001 + 1.0 / BEAT as f64).contains(&stopped),
        "{stopped}"
    );
}

#[test]
fn a_write_chained_to_a_ramp_at_its_rank_replaces_it() {
    use crate::control_request::{Condition, Predicate};
    let mut rig = ramp_rig();
    let when = on_beat(&rig, 4.0, 1.0);
    let ramp = submit_level(&rig, Source::Automation, 0.75, when, Some(2.0));
    // Applying on the ramp's own sample, at its own rank, it leaves the
    // dial's hold as the ramp took it: the write itself ends the ramp.
    let after = Condition::all(&[Predicate::Applied(ramp)]).unwrap();
    let then = submit_level(&rig, Source::Automation, 0.1, When::Condition(after), None);
    let out = render_counted(&mut rig, 64);
    let [(_, Outcome::Applied { at })] = outcomes_of(&mut rig, then)[..] else {
        panic!("the chained write applies");
    };
    assert_eq!(at as usize, beat_one());
    assert!(out[at as usize..].iter().all(|&level| level == 0.1));
}

/// Submits a write the dial refuses as it applies (a level must be a
/// number), at automation's rank, at `when`.
fn submit_refused(rig: &Rig, when: When) -> RequestId {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("dial", LEVEL).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::Bool(true)));
    request.when = when;
    request.source = Source::Automation;
    rig.live.requests.submit(request).unwrap()
}

/// A write the module refuses changes nothing: a ramp on its control
/// still reaches its end, whether the write was timed for a sample or on
/// a condition.
#[test]
fn a_refused_write_leaves_a_ramp_running() {
    use crate::control_request::{Condition, Predicate};
    for chained in [false, true] {
        let mut rig = ramp_rig();
        let when = on_beat(&rig, 4.0, 1.0);
        let ramp = submit_level(&rig, Source::Automation, 0.75, when, Some(2.0));
        let when = match chained {
            false => When::AtSample(2048),
            true => When::Condition(Condition::all(&[Predicate::Applied(ramp)]).unwrap()),
        };
        let refused = submit_refused(&rig, when);
        let out = render_counted(&mut rig, 64);
        let unsupported = Outcome::Refused(Refusal::Unsupported);
        assert_eq!(outcomes_of(&mut rig, refused), [(refused, unsupported)]);
        let end = beat_one() + 2 * BEAT;
        assert!(out[end..].iter().all(|&level| level == 0.75), "{chained}");
    }
}
