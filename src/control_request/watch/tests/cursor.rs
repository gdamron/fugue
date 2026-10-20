//! Tables played in place on a scripted timeline: where step 0 falls, the
//! sample each entry fires on, resets and new plans, holds, ramps and
//! their room, finding tables again, and passes that neither allocate nor
//! free.

use super::ramps::RampHost;
use super::*;
use crate::control_request::automation::Automation;
use crate::control_request::hold::Rank;
use crate::control_request::ramps::Ramps;
use crate::control_request::request::Source;
use crate::test_support::table::{entry, number, TestTable};

/// The table's id; it is at module 2, its clock at module 0.
const TABLE: usize = 7;

/// A clock a beat every 128 samples, `step` beats a gate.
fn clock(step: f64) -> Option<StepClock> {
    Some(StepClock {
        module: 0,
        id: 1,
        beats: step,
    })
}

/// A host at `position` whose module 2 plays `entries`.
fn host(position: f64, entries: Vec<crate::control_request::TableEntry>) -> RampHost {
    let mut host = RampHost::new(0.0);
    host.clock.position = position;
    host.table = Some(TestTable::new(TABLE, entries));
    host
}

/// A drain's cursors and ramps.
struct Drain {
    cursors: Cursors,
    ramps: Ramps,
    store: PendingStore,
    _outcomes: OutcomeReceiver,
    /// The longest segment it plays.
    segment: u64,
}

impl Drain {
    fn new(cursors: Cursors, ramps: Ramps) -> Self {
        let (store, _outcomes) = store();
        Self {
            cursors,
            ramps,
            store,
            _outcomes,
            segment: 64,
        }
    }
}

/// A drain with the table found on a clock `step` beats a gate, and room
/// for `ramp_room` ramps.
fn drain(step: f64, ramp_room: usize) -> Drain {
    let mut cursors = Cursors::new(4);
    let mut ramps = Ramps::new(ramp_room);
    assert!(cursors.found(TABLE, 2, clock(step), &mut ramps));
    Drain::new(cursors, ramps)
}

/// Plays to sample `until`, passing and stepping the ramps at each
/// segment start as the drain does, and records each change of what
/// `watched` hold as (sample, control, value).
fn play(
    drain: &mut Drain,
    host: &mut RampHost,
    until: u64,
    watched: &[&Automation],
    seen: &mut Vec<(u64, usize, f32)>,
) {
    let Drain {
        cursors,
        ramps,
        store,
        segment,
        ..
    } = drain;
    while host.now < until {
        while cursors.pass(host.now, host, ramps).0 {}
        let arrives = ramps.step(INSTALLED, host, &mut store.outcomes.retirer);
        let (_, wait) = cursors.pass(host.now, host, ramps);
        for (i, control) in watched.iter().enumerate() {
            let value = control.current().unwrap();
            let last = seen.iter().rev().find(|&&(_, of, _)| of == i);
            if last.is_none_or(|&(_, _, last)| last != value) {
                seen.push((host.now, i, value));
            }
        }
        let next = wait.unwrap_or(*segment).min(arrives.unwrap_or(*segment));
        let next = next.min(*segment);
        host.play(next.min(until - host.now));
    }
}

#[test]
fn step_0_is_the_clock_s_first_gate_and_each_entry_fires_on_its_step() {
    let level = number(0.0);
    let entries = vec![
        entry(0, 0, 0.25, &level),
        entry(1, 0, 0.5, &level),
        entry(3, 0, 0.75, &level),
    ];
    let mut host = host(BEFORE_START, entries);
    let mut drain = drain(1.0, 4);
    let mut seen = Vec::with_capacity(16);
    play(&mut drain, &mut host, 1024, &[&level], &mut seen);
    // Sample n is at position (n + 1) / 128.
    assert_eq!(seen, [(0, 0, 0.25), (127, 0, 0.5), (383, 0, 0.75)]);
}

#[test]
fn a_table_is_told_its_step_on_each_step_s_sample_with_or_without_entries() {
    use std::sync::atomic::Ordering;
    for entries in [0, 1] {
        let level = number(0.0);
        let table = (0..entries).map(|_| entry(1, 0, 0.5, &level)).collect();
        let mut host = host(BEFORE_START, table);
        let Drain { cursors, ramps, .. } = &mut drain(1.0, 4);
        let mut steps = Vec::with_capacity(16);
        while host.now < 600 {
            // In segments of 64 at most, split where the cursor waits.
            let (_, wait) = cursors.pass(host.now, &mut host, ramps);
            let step = host.table.as_ref().unwrap().step.load(Ordering::Relaxed);
            if steps.last().is_none_or(|&(_, last)| last != step) {
                steps.push((host.now, step));
            }
            host.play(wait.unwrap_or(64).min(64));
        }
        // Sample n is at position (n + 1) / 128: step k from sample 128k - 1.
        assert_eq!(steps, [(0, 0), (127, 1), (255, 2), (383, 3), (511, 4)]);
    }
}

#[test]
fn a_table_first_seen_mid_play_starts_on_the_clock_s_next_gate() {
    let level = number(0.0);
    let entries = vec![entry(0, 0, 0.25, &level), entry(1, 0, 0.5, &level)];
    // At beat 2.5, on a gate every half beat: step 0 is beat 3.
    let mut host = host(2.5, entries);
    let mut drain = drain(0.5, 4);
    let mut seen = Vec::with_capacity(16);
    play(&mut drain, &mut host, 1024, &[&level], &mut seen);
    assert_eq!(seen, [(0, 0, 0.0), (63, 0, 0.25), (127, 0, 0.5)]);
}

#[test]
fn a_reset_of_the_clock_plays_the_table_again_from_step_0() {
    let (level, swell) = (number(0.0), number(0.0));
    let entries = vec![
        entry(0, 0, 0.25, &level),
        entry(1, 4, 1.0, &swell),
        entry(2, 0, 0.5, &level),
    ];
    let mut host = host(BEFORE_START, entries);
    let mut drain = drain(1.0, 4);
    let mut seen = Vec::with_capacity(64);
    play(&mut drain, &mut host, 300, &[&level], &mut seen);
    assert_eq!(drain.ramps.running(), 1, "the swell glides");
    // Reset at 300: beat 3 is begun, and the clock's next sample is its
    // new beat 0.
    host.clock.beats_before += 3;
    host.clock.position = BEFORE_START;
    play(&mut drain, &mut host, 301, &[&level], &mut seen);
    assert!(drain.ramps.is_empty(), "the swell's ramp ended there");
    let swell_at = swell.current().unwrap();
    assert!(swell_at > 0.0 && swell_at < 1.0, "held where it was");
    play(&mut drain, &mut host, 600, &[&level], &mut seen);
    assert_eq!(drain.ramps.running(), 1, "the swell glides again");
    assert_eq!(
        seen,
        [(0, 0, 0.25), (255, 0, 0.5), (300, 0, 0.25), (555, 0, 0.5)]
    );
}

#[test]
fn another_plan_plays_on_from_the_step_the_clock_is_in() {
    let level = number(0.0);
    let entries = vec![entry(0, 0, 0.25, &level), entry(4, 0, 0.5, &level)];
    let mut host = host(BEFORE_START, entries);
    let mut drain = drain(1.0, 4);
    let mut seen = Vec::with_capacity(16);
    play(&mut drain, &mut host, 200, &[&level], &mut seen);
    // In step 1: its entry has had its gate, the next is step 2's.
    let table = host.table.as_mut().unwrap();
    table.plan += 1;
    table.entries = vec![
        entry(1, 0, 0.6, &level),
        entry(2, 0, 0.7, &level),
        entry(3, 0, 0.8, &level),
    ];
    play(&mut drain, &mut host, 1024, &[&level], &mut seen);
    assert_eq!(seen, [(0, 0, 0.25), (255, 0, 0.7), (383, 0, 0.8)]);
}

#[test]
fn a_user_write_holds_an_entry_s_control_until_its_next_entry() {
    let level = number(0.0);
    let entries = vec![
        entry(1, 0, 0.25, &level),
        entry(2, 0, 0.5, &level),
        entry(3, 0, 0.75, &level),
    ];
    let mut host = host(BEFORE_START, entries);
    let mut drain = drain(1.0, 4);
    let mut seen = Vec::with_capacity(16);
    let user = |due| Standing {
        rank: Rank::new(Source::User, 0),
        due,
    };
    // Due at step 1's sample: the entry there loses to it.
    level.cells.holds.take(level.index, user(127));
    play(&mut drain, &mut host, 200, &[&level], &mut seen);
    assert_eq!(seen, [(0, 0, 0.0)]);
    // Due before step 2's: the entry there takes the control back.
    level.cells.holds.take(level.index, user(200));
    play(&mut drain, &mut host, 1024, &[&level], &mut seen);
    assert_eq!(seen, [(0, 0, 0.0), (255, 0, 0.5), (383, 0, 0.75)]);
}

#[test]
fn a_ramp_replaced_as_a_user_s_late_write_lands_leaves_that_write() {
    let level = number(0.0);
    let entries = vec![entry(0, 4, 1.0, &level), entry(1, 2, 0.0, &level)];
    let mut host = host(BEFORE_START, entries);
    let mut drain = drain(1.0, 4);
    let mut seen = Vec::with_capacity(64);
    play(&mut drain, &mut host, 127, &[&level], &mut seen);
    // Taken late, in the pass that fires step 1: due at 126.
    level.write_number(0.8);
    let standing = Standing {
        rank: Rank::new(Source::User, 0),
        due: 126,
    };
    level.cells.holds.take(level.index, standing);
    play(&mut drain, &mut host, 128, &[&level], &mut seen);
    assert_eq!(seen.last(), Some(&(127, 0, 0.8)), "step 1 glides from it");
}

#[test]
fn a_ramp_entry_glides_to_land_on_its_end_step_or_writes_with_no_room() {
    let (a, b) = (number(0.0), number(0.0));
    let entries = vec![entry(1, 2, 1.0, &a), entry(1, 2, 1.0, &b)];
    let mut host = host(BEFORE_START, entries);
    // Room for one table ramp: b's writes its value at once.
    let mut drain = drain(1.0, 1);
    let mut seen = Vec::with_capacity(64);
    play(&mut drain, &mut host, 1024, &[&a, &b], &mut seen);
    let b_seen: Vec<_> = seen.iter().filter(|&&(_, of, _)| of == 1).collect();
    assert_eq!(b_seen, [&(0, 1, 0.0), &(127, 1, 1.0)]);
    let a_seen: Vec<_> = seen.iter().filter(|&&(_, of, _)| of == 0).collect();
    assert!(a_seen[1].0 > 127, "it leaves a where it stands at step 1");
    assert_eq!(**a_seen.last().unwrap(), (383, 0, 1.0), "landing on step 3");
    assert!(a_seen
        .windows(2)
        .all(|pair| pair[0].2 < pair[1].2 || pair[0].0 == 0));
}

#[test]
fn back_to_back_ramps_meet_on_their_shared_step_whatever_the_segments() {
    for segment in [64, 37, 128, 200, 1024] {
        let level = number(0.0);
        let entries = vec![entry(0, 1, 1.0, &level), entry(1, 1, 0.0, &level)];
        let mut host = host(BEFORE_START, entries);
        let mut drain = drain(1.0, 4);
        drain.segment = segment;
        let mut seen = Vec::with_capacity(64);
        play(&mut drain, &mut host, 512, &[&level], &mut seen);
        let peak = seen.iter().map(|&(_, _, value)| value).fold(0.0, f32::max);
        assert_eq!(peak, 1.0, "segments of {segment}");
        assert!(seen.contains(&(127, 0, 1.0)), "segments of {segment}");
        assert_eq!(seen.last(), Some(&(255, 0, 0.0)), "segments of {segment}");
    }
}

#[test]
fn tables_are_found_again_and_those_gone_end() {
    let level = number(0.0);
    let mut ramps = Ramps::new(4);
    let mut cursors = Cursors::new(1);
    assert!(!cursors.found(TABLE, 2, None, &mut ramps), "no clock");
    let no_step = clock(0.0);
    assert!(!cursors.found(TABLE, 2, no_step, &mut ramps));
    assert!(cursors.found(TABLE, 2, clock(1.0), &mut ramps));
    assert!(!cursors.found(TABLE + 1, 3, clock(1.0), &mut ramps), "full");

    let entries = vec![entry(0, 8, 1.0, &level), entry(1, 0, 0.5, &level)];
    let mut host = host(BEFORE_START, entries);
    let mut drain = Drain::new(cursors, ramps);
    let mut seen = Vec::with_capacity(16);
    play(&mut drain, &mut host, 64, &[&level], &mut seen);
    assert_eq!(drain.ramps.running(), 1);
    let Drain { cursors, ramps, .. } = &mut drain;
    // Found again on another clock: it starts over, ending its ramp.
    cursors.seek();
    let other = StepClock {
        id: 2,
        ..clock(1.0).unwrap()
    };
    assert!(cursors.found(TABLE, 2, Some(other), ramps));
    cursors.sought(ramps);
    assert_eq!((cursors.is_empty(), ramps.running()), (false, 0));
    // Not found again: it ends.
    cursors.seek();
    cursors.sought(ramps);
    assert!(cursors.is_empty());
}

#[test]
fn a_clock_told_apart_only_by_its_token_starts_the_table_over() {
    // The same module index and beats: only the token says the clock was
    // replaced, wherever the allocator put it.
    for (id, replayed) in [(1, false), (2, true)] {
        let (level, swell) = (number(0.0), number(0.0));
        let entries = vec![
            entry(0, 8, 1.0, &swell),
            entry(0, 0, 0.25, &level),
            entry(1, 0, 0.5, &level),
        ];
        let mut host = host(BEFORE_START, entries);
        let mut drain = drain(1.0, 4);
        let mut seen = Vec::with_capacity(64);
        play(&mut drain, &mut host, 300, &[&level], &mut seen);
        assert_eq!(drain.ramps.running(), 1);
        let Drain { cursors, ramps, .. } = &mut drain;
        cursors.seek();
        let found = StepClock {
            id,
            ..clock(1.0).unwrap()
        };
        assert!(cursors.found(TABLE, 2, Some(found), ramps));
        cursors.sought(ramps);
        assert_eq!(ramps.running(), usize::from(!replayed), "token {id}");
        play(&mut drain, &mut host, 1024, &[&level], &mut seen);
        // At sample 300 the next gate is beat 3, sample 383.
        let again = [(383, 0, 0.25), (511, 0, 0.5)];
        let expected: &[_] = if replayed { &again } else { &[] };
        assert_eq!(seen[2..], *expected, "token {id}");
    }
}

#[test]
fn a_table_whose_module_plays_another_table_writes_nothing() {
    let level = number(0.0);
    let mut host = host(BEFORE_START, vec![entry(0, 0, 0.25, &level)]);
    host.table.as_mut().unwrap().id = TABLE + 1;
    let mut drain = drain(1.0, 4);
    let mut seen = Vec::with_capacity(16);
    play(&mut drain, &mut host, 256, &[&level], &mut seen);
    assert_eq!(seen, [(0, 0, 0.0)]);
}

#[test]
fn playing_tables_neither_allocates_nor_frees() {
    let (a, b) = (number(0.0), number(0.0));
    let entries = vec![
        entry(0, 0, 0.25, &a),
        entry(1, 2, 1.0, &b),
        entry(2, 0, 0.5, &a),
        entry(3, 0, 0.0, &b),
    ];
    let mut host = host(BEFORE_START, entries);
    let mut drain = drain(1.0, 4);
    let mut seen = Vec::with_capacity(1024);
    let ((), allocs, frees) = allocator_events(|| {
        play(&mut drain, &mut host, 1024, &[&a, &b], &mut seen);
        host.clock.beats_before += 1;
        host.clock.position = BEFORE_START;
        play(&mut drain, &mut host, 2048, &[&a, &b], &mut seen);
    });
    assert_eq!((allocs, frees), (0, 0));
}
