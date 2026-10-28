//! One write stands on a control at a sample (see `control_request::staged`):
//! of the requests reaching it there, from any table, staged or applied at
//! once, the one due last, then of the higher rank, then received last. The
//! others settle at once. Each race here is heard against a twin given only
//! the write that stands, timed for the same sample.

use super::request_beats::{outcomes_of, render_counted};
use super::requests::outcomes;
use super::*;
use crate::control_request::{
    BeatSpec, BeatTime, Condition, ControlIndex, Outcome, Predicate, Refusal, Request, RequestId,
    RequestValue, RtValue, When,
};
use crate::payload::Payload;
use crate::test_support::dial::{DialFactory, DIAL, HELD, LEVEL};
use crate::test_support::tape::{Notes, TapeFactory, GAIN, NOTES, TAPE};

/// Blocks rendered: past beat 3.
const BLOCKS: usize = 56;

/// A clock beating every 1 024 samples, and a dial (level 0.25) and a tape
/// (`[0.25]`), all into an unclipped dac: the output is the clock's gate,
/// the level and the tape. The clock, added last, computes first, so the
/// dial reads its gate on time.
fn race_rig() -> Rig {
    let mut rig = Rig::new(
        r#"{
        "version": "1.0.0",
        "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
        "connections": []
    }"#,
    );
    rig.registry.register(DialFactory);
    rig.registry.register(TapeFactory);
    rig.adopt_registry();
    let dial = rig.build("dial", DIAL, serde_json::json!({}));
    let tape = rig.build("tape", TAPE, serde_json::json!({ "notes": [0.25] }));
    let clock = rig.build("clock", "clock", serde_json::json!({ "bpm": 2812.5 }));
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.upsert("tape", tape);
            change.upsert("clock", clock);
            change.connect(edge("clock", "beat", "dac", "audio"))?;
            change.connect(edge("dial", "out", "dac", "audio"))?;
            change.connect(edge("tape", "out", "dac", "audio"))
        })
        .unwrap();
    rig
}

/// How a write is timed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Timing {
    /// On the clock's beat 1 (`every` beats from then, when given).
    Beat(Option<f32>),
    /// From the clock's beat 1, gliding over so many beats.
    Glide(f32),
    /// On the clock gate's so manyth rising edge.
    Edge(u32),
    /// At the sample beat 1 falls on.
    Sample,
    /// When the write `Applied` names applies.
    Chained,
}

/// A write: its module and control, what it writes, and how it is timed.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Write(&'static str, ControlIndex, Value, Timing);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Value {
    Level(f32),
    Flag(bool),
    Notes(f32),
}

impl Write {
    fn timed(self, timing: Timing) -> Self {
        Self(self.0, self.1, self.2, timing)
    }
}

/// Submits `write`, beat 1 falling at `at`.
fn submit(rig: &Rig, Write(module, control, value, timing): Write, at: u64) -> RequestId {
    submit_after(rig, Write(module, control, value, timing), at, None)
}

/// [`submit`], `Chained` on request `on`.
fn submit_after(rig: &Rig, write: Write, at: u64, on: Option<RequestId>) -> RequestId {
    let Write(module, control, value, timing) = write;
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target(module, control).unwrap();
    let clock = publisher.control_target("clock", ControlIndex(0)).unwrap();
    let value = match value {
        Value::Level(level) => RequestValue::Value(RtValue::F32(level)),
        Value::Flag(flag) => RequestValue::Value(RtValue::Bool(flag)),
        Value::Notes(note) => RequestValue::Payload(Payload::new(Notes(vec![note], None))),
    };
    let mut request = Request::new(target, value);
    request.when = match timing {
        Timing::Beat(every) => When::Beat(BeatTime {
            clock: clock.module_idx as u32,
            spec: BeatSpec::Grid {
                every: every.unwrap_or(4.0),
                offset: 1.0,
                repeat: every.is_some(),
            },
            ramp: None,
        }),
        Timing::Glide(beats) => When::Beat(BeatTime {
            clock: clock.module_idx as u32,
            spec: BeatSpec::Grid {
                every: 4.0,
                offset: 1.0,
                repeat: false,
            },
            ramp: Some(beats),
        }),
        Timing::Edge(count) => {
            let edge = Predicate::edges(clock.module_idx as u32, 0, count);
            When::Condition(Condition::all(&[edge]).unwrap())
        }
        Timing::Sample => When::AtSample(at),
        Timing::Chained => {
            let on = Predicate::Applied(on.expect("chained on a request"));
            When::Condition(Condition::all(&[on]).unwrap())
        }
    };
    rig.live.requests.submit(request).unwrap()
}

/// The sample beat 1 falls on.
fn beat_one() -> u64 {
    let mut probe = race_rig();
    let id = submit(&probe, LEVEL_ON_BEAT, 0);
    render_counted(&mut probe, BLOCKS);
    let [(_, Outcome::Applied { at })] = outcomes_of(&mut probe, id)[..] else {
        panic!("beat 1 applies on time");
    };
    at
}

/// Renders `writes` submitted in order (each one `Chained` on the one
/// before), beat 1 falling at `at`, and a twin given only `stands`, timed
/// for `at`, and asserts they sound the same. Returns each write's
/// outcomes.
fn heard_as(writes: &[Write], stands: &[Write], at: u64) -> Vec<Vec<Outcome>> {
    let mut rig = race_rig();
    let mut ids = Vec::new();
    for &write in writes {
        ids.push(submit_after(&rig, write, at, ids.last().copied()));
    }
    let out = render_counted(&mut rig, BLOCKS);
    let mut twin = race_rig();
    for &write in stands {
        submit(&twin, write.timed(Timing::Sample), at);
    }
    assert_same(
        &out,
        &render_counted(&mut twin, BLOCKS),
        &format!("{writes:?}"),
    );
    let all = outcomes(&mut rig);
    ids.into_iter()
        .map(|id| {
            all.iter()
                .filter(|(got, _)| *got == id)
                .map(|(_, outcome)| *outcome)
                .collect()
        })
        .collect()
}

/// Asserts two renders sound the same, naming the first sample they part.
fn assert_same(out: &[f32], twin: &[f32], what: &str) {
    let parted = out.iter().zip(twin).position(|(a, b)| a != b);
    assert_eq!(
        parted,
        None,
        "{what}: {:?} against {:?}",
        parted.map(|at| out[at]),
        parted.map(|at| twin[at])
    );
}

const LEVEL_ON_BEAT: Write = Write("dial", LEVEL, Value::Level(0.5), Timing::Beat(None));
const LEVEL_AT: Write = Write("dial", LEVEL, Value::Level(0.75), Timing::Sample);
const NOTES_ON_BEAT: Write = Write("tape", NOTES, Value::Notes(0.5), Timing::Beat(None));
const NOTES_AT: Write = Write("tape", NOTES, Value::Notes(0.125), Timing::Sample);
const GAIN_AT: Write = Write("tape", GAIN, Value::Level(0.5), Timing::Sample);

/// Two writes on one control at a beat, either first: the one received
/// last stands. A beat's write applies at once, after the writes staged
/// before it and before those staged after: one received before it
/// applied, unheard, and one received after supersedes it. So does a
/// write on the clock's gate (applied as the dial processes, after the
/// beat's writes), and a ramp gives way whole. A payload rewriting another
/// control (the notes, the gain) loses only on its own, so a gain received
/// after it plays in the notes.
#[test]
fn of_writes_on_one_control_at_a_beat_the_one_received_last_stands() {
    let at = beat_one();
    let (applied, superseded) = (Outcome::Applied { at }, Outcome::Superseded);
    let on_gate = Write("dial", LEVEL, Value::Level(0.75), Timing::Edge(2));
    let glide = Write("dial", LEVEL, Value::Level(0.5), Timing::Glide(1.0));
    let races = [
        (LEVEL_ON_BEAT, LEVEL_AT, superseded),
        (LEVEL_AT, LEVEL_ON_BEAT, applied),
        (NOTES_ON_BEAT, NOTES_AT, superseded),
        (NOTES_AT, NOTES_ON_BEAT, applied),
        (GAIN_AT, NOTES_ON_BEAT, applied),
        (on_gate, LEVEL_ON_BEAT, superseded),
        (LEVEL_ON_BEAT, on_gate, applied),
        (glide, LEVEL_AT, superseded),
    ];
    for (older, newer, lost) in races {
        let settled = heard_as(&[older, newer], &[newer], at);
        assert_eq!(
            settled,
            [vec![lost], vec![applied]],
            "{older:?} then {newer:?}"
        );
    }
    let settled = heard_as(&[NOTES_ON_BEAT, GAIN_AT], &[NOTES_ON_BEAT, GAIN_AT], at);
    assert_eq!(
        settled,
        [vec![applied], vec![applied]],
        "the gain plays in the notes"
    );
}

/// A write a condition waits on applies at once, so the condition sees
/// it: one received after it for the same sample still stands, and the
/// chain breaks.
#[test]
fn a_write_a_chain_waits_on_gives_way_to_one_received_after_it() {
    let at = beat_one();
    let flag = Write("dial", HELD, Value::Flag(true), Timing::Chained);
    let mut rig = race_rig();
    let watched = submit(&rig, LEVEL_ON_BEAT, at);
    let chained = submit_after(&rig, flag, at, Some(watched));
    let newer = submit(&rig, LEVEL_AT, at);
    let out = render_counted(&mut rig, BLOCKS);
    let mut twin = race_rig();
    submit(&twin, LEVEL_AT, at);
    assert_same(&out, &render_counted(&mut twin, BLOCKS), "race");
    let all = outcomes(&mut rig);
    assert!(all.contains(&(watched, Outcome::Superseded)), "{all:?}");
    let broken = Outcome::Refused(Refusal::ChainBroken);
    assert!(all.contains(&(chained, broken)), "{all:?}");
    assert!(all.contains(&(newer, Outcome::Applied { at })), "{all:?}");
}

/// A write chained with no offset on one applied at a sample falls due
/// there once that one applies, after the pending store's writes there,
/// and stands against them by its receipt.
#[test]
fn a_write_chained_with_no_offset_stands_by_its_receipt() {
    let at = beat_one();
    let first = Write("dial", HELD, Value::Flag(true), Timing::Sample);
    let chained = Write("dial", LEVEL, Value::Level(0.5), Timing::Chained);
    // Received after the write it waits on, before the pending one.
    let mut rig = race_rig();
    let on = submit(&rig, first, at);
    let waiting = submit_after(&rig, chained, at, Some(on));
    let newer = submit(&rig, LEVEL_AT, at);
    let out = render_counted(&mut rig, BLOCKS);
    let mut twin = race_rig();
    submit(&twin, first, at);
    submit(&twin, LEVEL_AT, at);
    assert_same(&out, &render_counted(&mut twin, BLOCKS), "race");
    let all = outcomes(&mut rig);
    assert!(all.contains(&(waiting, Outcome::Superseded)), "{all:?}");
    assert!(all.contains(&(newer, Outcome::Applied { at })), "{all:?}");
    // Received after both: the one it waits on applies at once, as its
    // condition must see it, and flushes the pending write before it.
    let settled = heard_as(&[LEVEL_AT, first, chained], &[first, chained], at);
    assert_eq!(settled[2], [Outcome::Applied { at }]);
}

/// A repeating grid applies at once on each beat, and a write received
/// after it for one of them stands there: the grid plays on from its next.
#[test]
fn a_repeating_grid_gives_way_on_a_beat_to_a_write_received_after_it() {
    let at = beat_one();
    let grid = Write("dial", LEVEL, Value::Level(0.5), Timing::Beat(Some(2.0)));
    let mut rig = race_rig();
    let grid_id = submit(&rig, grid, at);
    submit(&rig, LEVEL_AT, at);
    let out = render_counted(&mut rig, BLOCKS);
    // The grid's next beat, two beats (1 024 samples each) on.
    let next = at + 2048;
    let mut twin = race_rig();
    submit(&twin, LEVEL_AT, at);
    submit(&twin, grid.timed(Timing::Sample), next);
    assert_same(&out, &render_counted(&mut twin, BLOCKS), "race");
    let all = outcomes(&mut rig);
    assert!(
        all.contains(&(grid_id, Outcome::Applied { at: next })),
        "{all:?}"
    );
}

/// A ramp starting on a beat glides from where a write received before
/// it for that sample leaves its control, as if each applied on time.
#[test]
fn a_ramp_glides_from_a_write_received_before_it_at_its_start() {
    let at = beat_one();
    let glide = Write("dial", LEVEL, Value::Level(0.5), Timing::Glide(1.0));
    let mut rig = race_rig();
    let first = submit(&rig, LEVEL_AT, at);
    let ramp = submit(&rig, glide, at);
    let out = render_counted(&mut rig, BLOCKS);
    // The twin's level stands at 0.75 from the start.
    let mut twin = race_rig();
    submit(&twin, LEVEL_AT, 0);
    submit(&twin, glide, at);
    let from = at as usize;
    let expected = render_counted(&mut twin, BLOCKS);
    assert_same(&out[from..], &expected[from..], "glide");
    let all = outcomes(&mut rig);
    assert!(all.contains(&(first, Outcome::Applied { at })), "{all:?}");
    assert!(all.contains(&(ramp, Outcome::Applied { at })), "{all:?}");
}
