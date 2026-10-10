//! Requests on a clock's beat grid on a live graph: the next multiple of
//! N beats (offset k) from any phase, and every N beats offset k,
//! repeating, across a reset, each on the exact sample by a twin clock.

use super::request_beats::{
    beat_rig, blocks_for, level_change, outcomes_of, render_counted, submit_clock_at, submit_level,
    submit_on, Twin, RESET,
};
use super::requests::{counted_block, outcomes};
use crate::control_request::{BeatSpec, Outcome, Refusal, RequestId, RtValue, Timeline};
use crate::test_support::dial::LEVEL;

/// A grid of `every` beats, offset `offset`.
fn grid(every: f32, offset: f32, repeat: bool) -> BeatSpec {
    BeatSpec::Grid {
        every,
        offset,
        repeat,
    }
}

/// The samples a repeating grid applies at, by the twin, until `end`.
fn grid_samples(twin: &mut Twin, every: f64, offset: f64, end: u64) -> Vec<u64> {
    let mut samples = Vec::new();
    while twin.sample < end {
        let before = twin.clock.position();
        twin.tick();
        let after = twin.clock.position();
        // A grid beat in (before, after], or beat 0 after a reset.
        let reached = if after < before {
            (offset.rem_euclid(every) == 0.0).then_some(())
        } else {
            let base = offset.rem_euclid(every);
            let m = ((after - base) / every).floor();
            (base + m * every > before && base + m * every <= after).then_some(())
        };
        if reached.is_some() {
            samples.push(twin.sample - 1);
        }
    }
    samples
}

#[test]
fn the_next_multiple_quantizes_from_any_phase() {
    for bpm in [97.0, 133.3] {
        for blocks in [0, 1, 5, 31, 60, 93] {
            for (every, offset) in [(4.0, 0.0), (4.0, 1.5), (2.0, -0.5), (0.75, 0.0)] {
                let mut rig = beat_rig(bpm);
                rig.render(blocks);
                let start = rig.graph.current_sample;
                let spec = grid(every, offset, false);
                let id = submit_level(&rig, 0.5, spec);
                render_counted(&mut rig, blocks_for(bpm, f64::from(every)));

                let mut twin = Twin::new(bpm);
                twin.run_to(start);
                let from = twin.clock.position();
                let (every, base) = (f64::from(every), f64::from(offset.rem_euclid(every)));
                let next = (0..)
                    .map(|m| base + m as f64 * every)
                    .find(|beat| *beat > from)
                    .unwrap();
                let at = twin.until(|clock| clock.position() >= next);
                let applied = Outcome::Applied { at };
                assert_eq!(
                    outcomes(&mut rig),
                    [(id, applied)],
                    "{bpm} bpm, from {from}, every {every} offset {offset}"
                );
            }
        }
    }
}

#[test]
fn every_n_beats_repeats_and_follows_a_reset() {
    for bpm in [97.0, 120.0] {
        for (every, offset) in [(2.0, 0.0), (3.0, 1.0)] {
            let mut rig = beat_rig(bpm);
            rig.render(4);
            let start = rig.graph.current_sample;
            let spec = grid(every, offset, true);
            let id = submit_level(&rig, 0.5, spec);
            let reset_at = start + 70_001;
            submit_clock_at(&rig, RESET, RtValue::Bool(true), reset_at);
            render_counted(&mut rig, blocks_for(bpm, 30.0));
            let end = rig.graph.current_sample;

            let mut twin = Twin::new(bpm);
            twin.writes.push((reset_at, RESET, RtValue::Bool(true)));
            twin.run_to(start);
            let expected: Vec<(RequestId, Outcome)> =
                grid_samples(&mut twin, every.into(), offset.into(), end)
                    .into_iter()
                    .map(|at| (id, Outcome::Applied { at }))
                    .collect();
            let got = outcomes_of(&mut rig, id);
            assert!(got.len() > 10, "{got:?}");
            assert_eq!(got, expected, "{bpm} bpm, every {every} offset {offset}");
        }
    }
}

#[test]
fn a_reset_timed_on_beats_loops_the_clock() {
    let mut rig = beat_rig(120.0);
    let id = submit_on(
        &rig,
        ("clock", RESET),
        RtValue::Bool(true),
        "clock",
        grid(3.0, 0.0, true),
        None,
    );
    let out = render_counted(&mut rig, blocks_for(120.0, 9.0));
    // Beat 0, then every 3 beats it starts over at beat 0, exactly 3 beats
    // later: gates every 24000 samples, and one reset at each loop start.
    let rises: Vec<usize> = (1..out.len())
        .filter(|&i| out[i] >= 1.0 && out[i - 1] < 1.0)
        .collect();
    assert!(
        rises.windows(2).all(|pair| pair[1] - pair[0] == 24_000),
        "{rises:?}"
    );
    let applied: Vec<u64> = outcomes(&mut rig)
        .into_iter()
        .map(|(got, outcome)| match outcome {
            Outcome::Applied { at } if got == id => at,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(&applied[..4], [0, 72_000, 144_000, 216_000]);
}

#[test]
fn the_next_multiple_lands_with_the_gate_rising() {
    let mut rig = beat_rig(97.0);
    rig.render(3);
    // From inside beat 0: the next multiple of 8 is the 8th rise.
    submit_level(&rig, 0.75, grid(8.0, 0.0, false));
    let out = render_counted(&mut rig, blocks_for(97.0, 9.0));
    let rises: Vec<usize> = (1..out.len())
        .filter(|&i| out[i] >= 1.0 && out[i - 1] < 1.0)
        .collect();
    assert_eq!(level_change(&out, 0.75), Some(rises[7]), "{rises:?}");
}

#[test]
fn removing_the_clock_ends_a_repeating_grid() {
    let mut rig = beat_rig(22_500.0);
    rig.render(1);
    let id = submit_level(&rig, 0.5, grid(1.0, 0.0, true));
    render_counted(&mut rig, 4);
    rig.live
        .edit(|change| {
            change.remove("clock");
            Ok(())
        })
        .unwrap();
    render_counted(&mut rig, 1);
    let got = outcomes_of(&mut rig, id);
    let (last, applied) = got.split_last().unwrap();
    assert_eq!(*last, (id, Outcome::Refused(Refusal::TimelineGone)));
    assert_eq!(applied.len(), 2, "{applied:?}");
}

#[test]
fn a_grid_no_clock_reaches_is_refused() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    let value = RtValue::F32(0.5);
    let bad = [grid(0.0, 0.0, true), grid(-4.0, 0.0, false)]
        .map(|spec| submit_on(&rig, ("dial", LEVEL), value, "clock", spec, None));
    render_counted(&mut rig, 1);
    let refused = Outcome::Refused(Refusal::Invalid);
    assert_eq!(outcomes(&mut rig), [(bad[0], refused), (bad[1], refused)]);
}

#[test]
fn a_repeating_grid_waits_and_applies_without_allocating() {
    let mut rig = beat_rig(22_500.0);
    rig.render(1);
    let id = submit_level(&rig, 0.5, grid(1.0, 0.0, true));
    for _ in 0..20 {
        assert_eq!(counted_block(&mut rig), (0, 0));
    }
    // A beat every 128 samples, each on the sample ending them: one every
    // other block, never early.
    let applied = outcomes(&mut rig);
    assert_eq!(applied.len(), 10, "{applied:?}");
    for (got, outcome) in applied {
        let Outcome::Applied { at } = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!((got, (at + 1) % 128), (id, 0));
    }
}
