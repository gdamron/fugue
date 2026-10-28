//! Conflicting writes on a live graph (A4): at one sample the higher tier,
//! then the higher priority, then the later write wins; a write above
//! automation holds its control until the lower tier's next event, so a
//! scheduler's ramp is abandoned until its next entry, which ramps on from
//! the held value. Every block stays allocation- and free-free.

use super::declared::dial_rig;
use super::migrated::{block_with_locks_held, schedule, write};
use super::request_beats::{beat_rig, level_change, render_counted, Twin};
use super::requests::{counted_block, outcomes};
use super::*;
use crate::control_request::{
    BeatSpec, BeatTime, ControlIndex, Outcome, Request, RequestId, RequestValue, RtValue, Source,
    Timeline, When,
};
use crate::test_support::dial::LEVEL;

const USER: (Source, i16) = (Source::User, 0);
const AGENT: (Source, i16) = (Source::Agent, 0);
const SCRIPT: (Source, i16) = (Source::Script, 0);

/// Submits `value` for the dial's level at sample `at`, from `source` at
/// `priority`.
fn submit_ranked(rig: &Rig, value: f32, at: u64, rank: (Source, i16)) -> RequestId {
    submit_value(rig, LEVEL, RtValue::F32(value), at, rank)
}

/// Submits `value` for the dial's `control` at sample `at`, from `source`
/// at `priority`.
fn submit_value(
    rig: &Rig,
    control: ControlIndex,
    value: RtValue,
    at: u64,
    (source, priority): (Source, i16),
) -> RequestId {
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("dial", control).unwrap();
    let mut request = Request::new(target, RequestValue::Value(value));
    request.when = When::AtSample(at);
    request.source = source;
    request.priority = priority;
    rig.live.requests.submit(request).unwrap()
}

pub(super) fn level(rig: &Rig) -> f32 {
    let (_, cells) = rig.graph.modules["dial"].module().declared().unwrap();
    match cells.load(LEVEL).unwrap() {
        RtValue::F32(level) => level,
        other => panic!("a number, not {other:?}"),
    }
}

fn overridden(rig: &Rig) -> u32 {
    let (_, cells) = rig.graph.modules["dial"].module().declared().unwrap();
    cells.automation.overridden().count()
}

/// The dial, its level ramped by a scheduler stepped by hand: 0 at step
/// 0, up to 1 over 8 steps from step 1, and from step 6 down to 0.1 over 2.
pub(super) fn ramp_rig() -> Rig {
    let mut rig = dial_rig();
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "dial", "control": "level", "value": 0.0 },
            { "at_step": 1, "module": "dial", "control": "level", "value": 1.0, "ramp_steps": 8 },
            { "at_step": 6, "module": "dial", "control": "level", "value": 0.1, "ramp_steps": 2 }
        ]}),
    );
    rig
}

/// One scheduler step (a block with the gate high, one with it low), each
/// block allocation- and free-free; the dial's level after it.
pub(super) fn step(rig: &mut Rig) -> f32 {
    for gate in [1.0, 0.0] {
        rig.live.write_input("sched", "clock", gate).unwrap();
        assert_eq!(counted_block(rig), (0, 0));
    }
    level(rig)
}

#[test]
fn a_user_write_mid_ramp_holds_until_the_next_automation_entry() {
    let mut rig = ramp_rig();
    let ramp: Vec<f32> = (0..4).map(|_| step(&mut rig)).collect();
    assert!(ramp.windows(2).all(|w| w[0] < w[1]), "ramping: {ramp:?}");

    write(&rig, "dial", "level", 0.9.into());
    assert_eq!((step(&mut rig), step(&mut rig)), (0.9, 0.9), "steps 4, 5");
    assert!(overridden(&rig) > 0);
    let (allocs, frees) = block_with_locks_held(&mut rig, &[]).unwrap();
    assert_eq!((allocs, frees, level(&rig)), (0, 0, 0.9));

    // Step 6's entry takes the control back, ramping from the held value.
    let resumed = step(&mut rig);
    assert!(resumed < 0.9 && resumed > 0.45, "{resumed}");
    step(&mut rig);
    assert_eq!(step(&mut rig), 0.1, "the entry's own ramp, to its end");
}

#[test]
fn a_script_write_holds_against_automation_but_not_against_a_user() {
    let mut rig = ramp_rig();
    for _ in 0..3 {
        step(&mut rig);
    }
    let now = rig.graph.current_sample;
    let script = submit_ranked(&rig, 0.8, now, SCRIPT);
    assert_eq!((step(&mut rig), step(&mut rig)), (0.8, 0.8), "steps 3, 4");
    assert_eq!(outcomes(&mut rig), [(script, Outcome::Applied { at: now })]);

    // A user takes it; the script's next request, later, takes it back.
    let now = rig.graph.current_sample;
    let user = submit_ranked(&rig, 0.3, now, USER);
    let again = submit_ranked(&rig, 0.6, now + 10, SCRIPT);
    let out = rig.render(1);
    assert_eq!((out[0], out[9], out[10]), (0.3, 0.3, 0.6));
    let applied = |at| Outcome::Applied { at };
    assert_eq!(
        outcomes(&mut rig),
        [(user, applied(now)), (again, applied(now + 10))]
    );
    // ...and holds against automation in turn, until step 6.
    assert_eq!(step(&mut rig), 0.6, "step 5");
    assert!(step(&mut rig) < 0.6, "step 6 resumes");
}

#[test]
fn an_entry_at_a_user_writes_sample_loses_to_it_and_holds_off_its_ramp() {
    let mut rig = ramp_rig();
    for _ in 0..6 {
        step(&mut rig);
    }
    // Due at the block step 6's entry fires in: the user's write wins.
    let now = rig.graph.current_sample;
    let user = submit_ranked(&rig, 0.9, now, USER);
    assert_eq!(step(&mut rig), 0.9, "step 6");
    assert_eq!(outcomes(&mut rig), [(user, Outcome::Applied { at: now })]);
    // Its ramp is abandoned, and no entry follows to take the control back.
    assert_eq!((step(&mut rig), step(&mut rig)), (0.9, 0.9), "steps 7, 8");
}

#[test]
fn an_abandoned_ramp_stays_abandoned_once_the_hold_is_gone() {
    let mut rig = ramp_rig();
    for _ in 0..4 {
        step(&mut rig);
    }
    write(&rig, "dial", "level", 0.9.into());
    assert_eq!(step(&mut rig), 0.9, "step 4: the ramp's writes dropped");
    // An automation-tier request frees the control; the ramp is gone.
    let now = rig.graph.current_sample;
    submit_ranked(&rig, 0.5, now, (Source::Automation, 0));
    assert_eq!(step(&mut rig), 0.5, "step 5");
}

#[test]
fn at_one_sample_the_higher_tier_wins_whatever_the_order() {
    let mut rig = dial_rig();
    let at = rig.graph.current_sample + 5;
    let script = submit_ranked(&rig, 0.5, at, (Source::Script, i16::MAX));
    let user = submit_ranked(&rig, 0.75, at, USER);
    let late = submit_ranked(&rig, 0.125, at, SCRIPT);
    // Past that sample, the script's next request takes the control back.
    let next = submit_ranked(&rig, 0.25, at + 1, SCRIPT);
    assert_eq!(block_with_locks_held(&mut rig, &[]), Ok((0, 0)));
    assert_eq!(level(&rig), 0.25);
    assert_eq!(
        outcomes(&mut rig),
        [
            (script, Outcome::Overridden),
            (late, Outcome::Overridden),
            (user, Outcome::Applied { at }),
            (next, Outcome::Applied { at: at + 1 }),
        ]
    );
}

#[test]
fn explicit_priority_ranks_writes_within_a_tier() {
    let mut rig = dial_rig();
    let at = rig.graph.current_sample;
    let high = submit_ranked(&rig, 0.5, at, (Source::Script, 5));
    let low = submit_ranked(&rig, 0.75, at, (Source::Script, -1));
    let higher = submit_ranked(&rig, 0.125, at, (Source::Script, 6));
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(level(&rig), 0.125);
    assert_eq!(
        outcomes(&mut rig),
        [
            (low, Outcome::Superseded),
            (high, Outcome::Superseded),
            (higher, Outcome::Applied { at }),
        ]
    );
}

#[test]
fn at_equal_tier_and_priority_the_last_write_wins() {
    let mut rig = dial_rig();
    let at = rig.graph.current_sample;
    let agent = submit_ranked(&rig, 0.5, at, AGENT);
    let user = submit_ranked(&rig, 0.75, at, USER);
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(level(&rig), 0.75);
    let applied = Outcome::Applied { at };
    assert_eq!(
        outcomes(&mut rig),
        [(agent, Outcome::Superseded), (user, applied)]
    );
}

#[test]
fn a_beat_lost_to_a_higher_tier_is_only_that_beat() {
    let bpm = 120.0;
    let mut rig = beat_rig(bpm);
    rig.render(10);
    let start = rig.graph.current_sample;
    let mut twin = Twin::new(bpm);
    twin.run_to(start);
    let next = twin.clock.position().floor() + 1.0;
    let first = twin.until(|clock| clock.position() >= next);
    let second = twin.until(|clock| clock.position() >= next + 1.0);

    let grid = BeatSpec::Grid {
        every: 1.0,
        offset: 0.0,
        repeat: true,
    };
    let script = {
        let publisher = rig.live.publisher().lock().unwrap();
        let target = publisher.control_target("dial", LEVEL).unwrap();
        let clock = publisher.control_target("clock", ControlIndex(0)).unwrap();
        let mut request = Request::new(target, RequestValue::Value(RtValue::F32(0.5)));
        request.when = When::Beat(BeatTime {
            clock: clock.module_idx as u32,
            spec: grid,
            ramp: None,
        });
        request.source = Source::Script;
        rig.live.requests.submit(request).unwrap()
    };
    let user = submit_ranked(&rig, 0.75, first, USER);
    let blocks = ((second - start) / 64 + 2) as usize;
    let out = render_counted(&mut rig, blocks);
    let at = |sample: u64| Some((sample - start) as usize);
    assert_eq!(
        level_change(&out, 0.75),
        at(first),
        "the user's, on the beat"
    );
    assert_eq!(level_change(&out, 0.5), at(second), "the grid's next beat");
    // The grid settles at the first beat it applied on.
    assert_eq!(
        outcomes(&mut rig),
        [
            (user, Outcome::Applied { at: first }),
            (script, Outcome::Applied { at: second })
        ]
    );
}

#[test]
fn a_late_write_is_weighed_at_its_due_sample() {
    let mut rig = dial_rig();
    let at = rig.graph.current_sample + 5;
    let user = submit_ranked(&rig, 0.75, at, USER);
    assert_eq!(counted_block(&mut rig), (0, 0));
    // A block late: one due before the user's write loses to it (on time,
    // the user's would have replaced it), and one due after it is the
    // script's next event.
    let late = submit_ranked(&rig, 0.5, at - 2, SCRIPT);
    let next = submit_ranked(&rig, 0.25, at + 1, SCRIPT);
    let now = rig.graph.current_sample;
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(level(&rig), 0.25);
    assert_eq!(
        outcomes(&mut rig),
        [
            (user, Outcome::Applied { at }),
            (late, Outcome::Overridden),
            (
                next,
                Outcome::AppliedLate {
                    at: now,
                    due: at + 1
                }
            ),
        ]
    );
}

#[test]
fn a_hold_stays_with_a_surviving_instance_and_never_reaches_a_rebuilt_one() {
    let mut rig = dial_rig();
    let holds = |rig: &Rig| {
        let (_, cells) = rig.graph.modules["dial"].module().declared().unwrap();
        cells.holds.holds_off_automation(LEVEL)
    };
    let edit = |rig: &mut Rig, id: &str| {
        let dial = rig.build(id, crate::test_support::dial::DIAL, serde_json::json!({}));
        rig.live
            .edit(|change| {
                change.upsert(id, dial);
                Ok(())
            })
            .unwrap();
    };
    // A user write and a script one at the same sample, an edit between.
    let at = rig.graph.current_sample;
    let user = submit_ranked(&rig, 0.75, at, USER);
    edit(&mut rig, "other");
    let script = submit_ranked(&rig, 0.5, at, SCRIPT);
    rig.render(1);
    assert!(holds(&rig), "the surviving dial keeps its hold");
    let (got, _) = rig.live.take_outcomes();
    assert_eq!(got, [(user, applied_at(at)), (script, Outcome::Overridden)]);

    let at = rig.graph.current_sample;
    let user = submit_ranked(&rig, 0.75, at, USER);
    edit(&mut rig, "dial");
    let script = submit_ranked(&rig, 0.5, at, SCRIPT);
    rig.render(1);
    assert_eq!(level(&rig), 0.5, "the rebuilt dial was never held");
    let (got, _) = rig.live.take_outcomes();
    assert_eq!(got, [(user, applied_at(at)), (script, applied_at(at))]);
}

fn applied_at(at: u64) -> Outcome {
    Outcome::Applied { at }
}

/// The session log records what applied: a write that lost to a higher
/// tier never did, so it is not logged, and a replay applies the winner.
#[test]
fn an_overridden_write_is_not_logged() {
    let mut rig = dial_rig();
    let at = rig.graph.current_sample + 5;
    let level = ("dial", "level");
    let value = |v| RtValue::F32(v);
    let script = Source::Script;
    rig.live
        .submit_logged_from(level, value(0.5), When::AtSample(at), None, script);
    rig.live
        .submit_logged_from(level, value(0.75), When::AtSample(at), None, Source::User);
    assert_eq!(counted_block(&mut rig), (0, 0));
    let log = rig.live.request_log();
    let logged: Vec<_> = log
        .requests
        .iter()
        .map(|r| (r.sample, r.value.clone()))
        .collect();
    assert_eq!(logged, [(at, crate::LoggedValue::Number(0.75))]);
    assert_eq!((log.lost, log.unlogged), (0, 0));
}

/// An automation-tier request takes its control back from a hold, which a
/// replay (holding every logged write above automation) cannot do: it is
/// counted unlogged, so the log says it cannot replay exactly.
#[test]
fn an_applied_automation_tier_request_is_counted_unlogged() {
    let mut rig = dial_rig();
    let at = rig.graph.current_sample;
    let target = ("dial", "level");
    let automated = Source::Automation;
    rig.live.submit_logged_from(
        target,
        RtValue::F32(0.5),
        When::AtSample(at),
        None,
        automated,
    );
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(level(&rig), 0.5, "applied");
    let log = rig.live.request_log();
    assert_eq!((log.requests.len(), log.unlogged), (0, 1));
}
