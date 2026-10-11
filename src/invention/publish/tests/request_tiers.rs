//! Conflicting writes on a live graph (A4): at one sample the higher tier,
//! then the higher priority, then the later write wins, and the loser is
//! reported overridden or superseded; past that sample a lower tier's next
//! request takes the control back. Every block stays allocation- and
//! free-free.

use super::declared::dial_rig;
use super::migrated::block_with_locks_held;
use super::requests::{counted_block, outcomes};
use super::*;
use crate::control_request::{
    ControlIndex, Outcome, Request, RequestId, RequestValue, RtValue, Source, When,
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

fn level(rig: &Rig) -> f32 {
    let (_, cells) = rig.graph.modules["dial"].module().declared().unwrap();
    match cells.load(LEVEL).unwrap() {
        RtValue::F32(level) => level,
        other => panic!("a number, not {other:?}"),
    }
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
    let logged: Vec<_> = log.requests.iter().map(|r| (r.sample, r.value)).collect();
    assert_eq!(logged, [(at, crate::LoggedValue::Number(0.75))]);
    assert_eq!((log.lost, log.unlogged), (0, 0));
}
