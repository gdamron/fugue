//! Writes to a module an immediate edit keeps, either side of the edit at
//! one sample, play as they would with no edit between them: one write
//! stands, by due sample, rank and receipt (see `control_request::staged`).
//! Each sequence is heard at the dac against a twin without the edit.

use super::request_beats::{render_counted, submit_on};
use super::requests::outcomes;
use super::*;
use crate::control_request::{
    BeatSpec, Condition, Edge, Outcome, Predicate, Request, RequestValue, RtValue, When,
};
use crate::test_support::dial::{DialFactory, DIAL, LEVEL};

/// Blocks rendered after the edit: past the clock's next beat.
const BLOCKS: usize = 400;

/// How a step is timed, from the sample everything is submitted at.
#[derive(Clone, Copy, Debug)]
enum Due {
    /// So many samples before it.
    Late(u64),
    /// So many samples after it.
    Ahead(u64),
    Now,
    /// On the clock.
    Beat(BeatSpec),
}

/// One step of a sequence.
#[derive(Clone, Copy)]
enum Step {
    /// A level for a dial.
    Level(&'static str, f32, Due),
    /// An edit; the twin leaves it out.
    Edit(fn(&Rig)),
}

/// A clock at 120 bpm the dac does not hear, a dial (level 0.25, clamped
/// to 0..=1) into the dac, and two dials it does not hear: `spare` and
/// `gone`.
fn edit_rig() -> Rig {
    let mut rig = Rig::new(
        r#"{
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": []
    }"#,
    );
    rig.registry.register(DialFactory);
    rig.adopt_registry();
    let [dial, spare, gone] =
        ["dial", "spare", "gone"].map(|id| rig.build(id, DIAL, serde_json::json!({})));
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.upsert("spare", spare);
            change.upsert("gone", gone);
            change.connect(edge("dial", "out", "dac", "audio"))
        })
        .unwrap();
    rig.render(4);
    rig
}

fn submit_level(rig: &Rig, module: &str, level: f32, due: Due, start: u64) -> RequestId {
    let value = RtValue::F32(level);
    let when = match due {
        Due::Late(by) => When::AtSample(start - by),
        Due::Ahead(by) => When::AtSample(start + by),
        Due::Now => When::Now,
        Due::Beat(spec) => return submit_on(rig, (module, LEVEL), value, "clock", spec, None),
    };
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target(module, LEVEL).unwrap();
    let mut request = Request::new(target, RequestValue::Value(value));
    request.when = when;
    rig.live.requests.submit(request).unwrap()
}

/// Plays `steps` from one block start (the edits only `with_edits`), and
/// returns the output and each level's outcomes, in order.
fn play(steps: &[Step], with_edits: bool) -> (Vec<f32>, Vec<Vec<Outcome>>) {
    let mut rig = edit_rig();
    let start = rig.graph.current_sample;
    let mut ids = Vec::new();
    for step in steps {
        match *step {
            Step::Level(module, level, due) => {
                ids.push(submit_level(&rig, module, level, due, start))
            }
            Step::Edit(edit) if with_edits => edit(&rig),
            Step::Edit(_) => {}
        }
    }
    let out = render_counted(&mut rig, BLOCKS);
    (out, settled(&mut rig, ids))
}

/// How each of `ids` settled, in order.
fn settled(rig: &mut Rig, ids: Vec<RequestId>) -> Vec<Vec<Outcome>> {
    let all = outcomes(rig);
    let of = |id| {
        all.iter()
            .filter(|(got, _)| *got == id)
            .map(|(_, o)| *o)
            .collect()
    };
    ids.into_iter().map(of).collect()
}

/// Asserts `steps` sound and settle as they would without their edits.
fn as_without_edits(steps: &[Step]) {
    assert_eq!(sounds_as_without_edits(steps), play(steps, false).1);
}

/// Asserts `steps` sound as they would without their edits, returning
/// how they settle.
fn sounds_as_without_edits(steps: &[Step]) -> Vec<Vec<Outcome>> {
    let (out, settled) = play(steps, true);
    let (twin, _) = play(steps, false);
    let parted = out.iter().zip(&twin).position(|(a, b)| a != b);
    assert_eq!(parted, None, "parted at {parted:?}");
    settled
}

fn rebuild_spare(rig: &Rig) {
    let spare = rig.build("spare", DIAL, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("spare", spare);
            Ok(())
        })
        .unwrap();
}

fn rebuild_spare_remove_gone(rig: &Rig) {
    let spare = rig.build("spare", DIAL, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("spare", spare);
            change.remove("gone");
            Ok(())
        })
        .unwrap();
}

fn rebuild_clock(rig: &Rig) {
    let clock = rig.build("clock", "clock", serde_json::json!({ "bpm": 120.0 }));
    rig.live
        .edit(|change| {
            change.upsert("clock", clock);
            Ok(())
        })
        .unwrap();
}

/// A late write, one due now, an edit keeping their module, then another
/// due now: the last stands.
#[test]
fn a_write_due_now_is_not_overtaken_across_an_edit_by_a_late_one() {
    as_without_edits(&[
        Step::Level("dial", 0.1, Due::Late(2)),
        Step::Level("dial", 0.2, Due::Now),
        Step::Edit(rebuild_spare),
        Step::Level("dial", 0.3, Due::Now),
    ]);
}

/// Late writes to a rebuilt, a removed and a kept module, the edit, then
/// one due earlier behind it on the kept: the kept module's write due
/// last stands.
#[test]
fn a_late_write_behind_an_edit_gives_way_to_one_due_after_it() {
    let steps = [
        Step::Level("spare", 0.1, Due::Late(3)),
        Step::Level("gone", 0.2, Due::Late(3)),
        Step::Level("dial", 0.3, Due::Late(3)),
        Step::Edit(rebuild_spare_remove_gone),
        Step::Level("dial", 0.4, Due::Late(5)),
    ];
    as_without_edits(&steps);
}

/// A write due now, the edit, then one due long before it: the first stands.
#[test]
fn a_write_behind_an_edit_does_not_reach_back_over_one_due_after_it() {
    as_without_edits(&[
        Step::Level("dial", 0.2, Due::Now),
        Step::Edit(rebuild_spare),
        Step::Level("dial", 0.1, Due::Late(5)),
    ]);
}

/// A late write, then a beat's write on a clock the edit rebuilds: the
/// beat's, due last, stands.
#[test]
fn a_beat_write_on_a_clock_an_edit_rebuilds_stands_over_a_late_write() {
    as_without_edits(&[
        Step::Level("dial", 0.25, Due::Late(2)),
        Step::Level("dial", 0.5, Due::Beat(BeatSpec::After(0.0))),
        Step::Edit(rebuild_clock),
    ]);
}

fn remove_clock(rig: &Rig) {
    rig.live
        .edit(|change| {
            change.remove("clock");
            Ok(())
        })
        .unwrap();
}

/// A late write, a beat's write on a clock the edit removes, then one due
/// now: the last stands, and the beat's, applied at once, reports Applied
/// (two setters at one sample, unheard, as without the edit).
#[test]
fn a_write_behind_an_edit_removing_a_clock_stands_over_its_beat_write() {
    let settled = sounds_as_without_edits(&[
        Step::Level("dial", 0.1, Due::Late(2)),
        Step::Level("dial", 0.2, Due::Beat(BeatSpec::After(0.0))),
        Step::Edit(remove_clock),
        Step::Level("dial", 0.3, Due::Now),
    ]);
    assert_eq!(settled[2], [Outcome::Applied { at: 256 }]);
}

/// A beat's write on a clock the edit removes, then a write behind the
/// edit due before it: the beat's stands (the late one reports
/// Superseded, where without the edit it applies first).
#[test]
fn a_late_write_behind_an_edit_removing_a_clock_gives_way_to_its_beat_write() {
    let settled = sounds_as_without_edits(&[
        Step::Level("dial", 0.2, Due::Beat(BeatSpec::After(0.0))),
        Step::Edit(remove_clock),
        Step::Level("dial", 0.1, Due::Late(5)),
    ]);
    assert_eq!(settled[1], [Outcome::Superseded]);
}

/// A crossing written due now, then an edit keeping its module: a
/// condition reads it as the next segment starts, as with no edit (the
/// pass after the install reads controls before what is staged applies).
#[test]
fn a_crossing_written_before_an_edit_is_read_as_without_it() {
    let met = |with_edit: bool| {
        let mut rig = edit_rig();
        let publisher = rig.live.publisher().lock().unwrap();
        let dial = publisher.control_target("dial", LEVEL).unwrap().module_idx;
        let target = publisher.control_target("gone", LEVEL).unwrap();
        drop(publisher);
        let crossing = Predicate::Control {
            module: dial as u32,
            control: LEVEL,
            edge: Edge::Rising,
            level: 0.5,
        };
        let mut request = Request::new(target, RequestValue::Value(RtValue::F32(0.5)));
        request.when = When::Condition(Condition::all(&[crossing]).unwrap());
        let waits = rig.live.requests.submit(request).unwrap();
        render_counted(&mut rig, 1);
        let start = rig.graph.current_sample;
        submit_level(&rig, "dial", 0.75, Due::Now, start);
        if with_edit {
            rebuild_spare(&rig);
        }
        render_counted(&mut rig, 4);
        let all = outcomes(&mut rig);
        all.into_iter().find(|(id, _)| *id == waits).map(|(_, o)| o)
    };
    let twin = met(false);
    assert!(matches!(twin, Some(Outcome::Applied { .. })), "{twin:?}");
    assert_eq!(met(true), twin);
}

/// A write due now, an edit keeping its module, then (if `replaced`)
/// another due now, and one on the same control waiting for the first to
/// apply: played with and without the edit, with when it started and how
/// each settled.
fn waiting_on_a_write_staged_before_an_edit(
    replaced: bool,
    with_edit: bool,
) -> (u64, Vec<f32>, Vec<Vec<Outcome>>) {
    let mut rig = edit_rig();
    let start = rig.graph.current_sample;
    let mut ids = vec![submit_level(&rig, "dial", 0.75, Due::Now, start)];
    if with_edit {
        rebuild_spare(&rig);
    }
    if replaced {
        ids.push(submit_level(&rig, "dial", 0.25, Due::Now, start));
    }
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("dial", LEVEL).unwrap();
    drop(publisher);
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(0.5)));
    let applied = Condition::all(&[Predicate::Applied(ids[0])]).unwrap();
    request.when = When::Condition(applied);
    ids.push(rig.live.requests.submit(request).unwrap());
    let out = render_counted(&mut rig, 4);
    (start, out, settled(&mut rig, ids))
}

/// The one waiting applies with the first, as with no edit (the first,
/// staged before the condition started, is flushed for it).
#[test]
fn a_write_waiting_on_one_staged_before_an_edit_applies_with_it() {
    let (start, twin, without) = waiting_on_a_write_staged_before_an_edit(false, false);
    assert_eq!(without[1], [Outcome::Applied { at: start }]);
    let (_, out, settled) = waiting_on_a_write_staged_before_an_edit(false, true);
    assert_eq!(out, twin);
    assert_eq!(settled, without);
}

/// Replaced behind the edit, the first never applies, so the one waiting
/// on it never does, as with no edit (it is weighed before it is flushed).
#[test]
fn a_write_waiting_on_one_replaced_behind_an_edit_never_applies() {
    let (_, twin, without) = waiting_on_a_write_staged_before_an_edit(true, false);
    assert_eq!(without[0], [Outcome::Superseded]);
    assert!(
        !matches!(without[2][..], [Outcome::Applied { .. }]),
        "{without:?}"
    );
    let (_, out, settled) = waiting_on_a_write_staged_before_an_edit(true, true);
    assert_eq!(out, twin);
    assert_eq!(settled, without);
}

/// An edit held two blocks, waiting for room to retire what it replaces:
/// writes due now and 70 samples on before it, then behind it one due five
/// samples before the first, a second edit, and one due 20 samples on,
/// which apply once both install. Each written before them and due later still
/// stands, as with no edit (once the second is due); a late write
/// submitted after that applies as it arrives, as any does.
#[test]
fn a_late_write_behind_a_held_edit_does_not_reach_back() {
    let play = |with_edit: bool| {
        let mut rig = edit_rig();
        let (osc1, osc2) = (
            rig.build("osc1", "oscillator", serde_json::json!({})),
            rig.build("osc2", "oscillator", serde_json::json!({})),
        );
        rig.live
            .edit(|change| {
                change.upsert("osc1", osc1);
                change.upsert("osc2", osc2);
                Ok(())
            })
            .unwrap();
        rig.render(1);
        rig.hold_a_retirement();
        let start = rig.graph.current_sample;
        let mut ids = vec![
            submit_level(&rig, "dial", 0.2, Due::Now, start),
            submit_level(&rig, "dial", 0.3, Due::Ahead(70), start),
        ];
        if with_edit {
            let spare = rig.build("spare", DIAL, serde_json::json!({}));
            rig.publish_unreclaimed(|change| change.upsert("spare", spare));
        }
        ids.push(submit_level(&rig, "dial", 0.1, Due::Late(5), start));
        if with_edit {
            let spare = rig.build("spare", DIAL, serde_json::json!({}));
            rig.publish_unreclaimed(|change| change.upsert("spare", spare));
        }
        ids.push(submit_level(&rig, "dial", 0.15, Due::Ahead(20), start));
        let mut out = render_counted(&mut rig, 2);
        rig.live.reclaim();
        out.extend(render_counted(&mut rig, 4));
        ids.push(submit_level(&rig, "dial", 0.4, Due::Late(5), start));
        out.extend(render_counted(&mut rig, 4));
        (out, settled(&mut rig, ids))
    };
    let (out, settled) = play(true);
    assert_eq!(out[70..], play(false).0[70..]);
    assert_eq!(
        settled[2..4],
        [[Outcome::Superseded], [Outcome::Superseded]]
    );
    assert!(matches!(settled[4][..], [Outcome::AppliedLate { .. }]));
}

/// Nine edits behind one held two blocks, past the installs a block takes:
/// a write due 20 samples on behind the ninth does not reach back over one
/// due 70 samples on before them, which applied while the first waited.
#[test]
fn a_write_behind_edits_held_past_one_block_does_not_reach_back() {
    let play = |with_edits: bool| {
        let mut rig = edit_rig();
        let (osc1, osc2) = (
            rig.build("osc1", "oscillator", serde_json::json!({})),
            rig.build("osc2", "oscillator", serde_json::json!({})),
        );
        rig.live
            .edit(|change| {
                change.upsert("osc1", osc1);
                change.upsert("osc2", osc2);
                Ok(())
            })
            .unwrap();
        rig.render(1);
        rig.hold_a_retirement();
        let start = rig.graph.current_sample;
        submit_level(&rig, "dial", 0.3, Due::Ahead(70), start);
        for _ in 0..(with_edits as usize * (MAX_INSTALLS_PER_BLOCK + 1)) {
            let spare = rig.build("spare", DIAL, serde_json::json!({}));
            rig.publish_unreclaimed(|change| change.upsert("spare", spare));
        }
        let behind = submit_level(&rig, "dial", 0.15, Due::Ahead(20), start);
        let mut out = render_counted(&mut rig, 2);
        for _ in 0..3 {
            rig.live.reclaim();
            out.extend(render_counted(&mut rig, 1));
        }
        out.extend(render_counted(&mut rig, 4));
        (out, settled(&mut rig, vec![behind]))
    };
    let (out, settled) = play(true);
    assert_eq!(out[70..], play(false).0[70..]);
    assert_eq!(settled, [[Outcome::Superseded]]);
}
