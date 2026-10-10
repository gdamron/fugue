//! Requests on a clock's beat grid on a live graph: the next multiple of
//! N beats (offset k) from any phase, and every N beats offset k,
//! repeating, across a reset, each on the exact sample by a twin clock.

use super::request_beats::{
    beat_rig, blocks_for, level_change, outcomes_of, render_counted, submit_clock_at, submit_level,
    submit_on, Twin, RESET,
};
use super::requests::outcomes;
use super::Rig;
use crate::control_request::{
    BeatSpec, ControlIndex, Outcome, Refusal, RequestId, RtValue, Timeline,
};
use crate::test_support::dial::{LEVEL, PULSE};
use crate::Module;

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

/// Pulses the dial at `spec` on the clock: each application adds one to
/// its output for good, so the output shows every one.
fn submit_pulse(rig: &Rig, spec: BeatSpec) -> RequestId {
    submit_on(
        rig,
        ("dial", PULSE),
        RtValue::Bool(true),
        "clock",
        spec,
        None,
    )
}

/// The samples, from `start`, at which the dial's pulse count rises in
/// `out` (the beat gate, by a twin with `writes`, taken away).
fn pulse_samples(
    out: &[f32],
    start: u64,
    bpm: f64,
    writes: Vec<(u64, ControlIndex, RtValue)>,
) -> Vec<u64> {
    let mut twin = Twin::new(bpm);
    twin.writes = writes;
    twin.run_to(start);
    let (mut count, mut samples) = (0.0, Vec::new());
    for (i, sample) in out.iter().enumerate() {
        twin.tick();
        let pulses = (sample - 0.25 - twin.clock.output_block(0)[0]).round();
        if pulses > count {
            assert_eq!(pulses, count + 1.0, "one pulse a sample");
            samples.push(start + i as u64);
            count = pulses;
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
            let id = submit_pulse(&rig, grid(every, offset, true));
            let reset_at = start + 70_001;
            submit_clock_at(&rig, RESET, RtValue::Bool(true), reset_at);
            let out = render_counted(&mut rig, blocks_for(bpm, 30.0));
            let end = rig.graph.current_sample;

            let writes = vec![(reset_at, RESET, RtValue::Bool(true))];
            let mut twin = Twin::new(bpm);
            twin.writes = writes.clone();
            twin.run_to(start);
            let expected = grid_samples(&mut twin, every.into(), offset.into(), end);
            let context = format!("{bpm} bpm, every {every} offset {offset}");
            assert!(expected.len() > 10, "{context}: {expected:?}");
            assert_eq!(
                pulse_samples(&out, start, bpm, writes),
                expected,
                "{context}"
            );
            // One outcome: its first application.
            let first = Outcome::Applied { at: expected[0] };
            assert_eq!(outcomes_of(&mut rig, id), [(id, first)], "{context}");
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
        grid(2.5, 0.0, true),
        None,
    );
    let out = render_counted(&mut rig, blocks_for(120.0, 9.0));
    // Beat 0, then every 2.5 beats (60000 samples) it starts over at beat
    // 0: gates at 0, 1 and 2 beats into each loop, to the sample.
    let rises: Vec<usize> = (1..out.len())
        .filter(|&i| out[i] >= 1.0 && out[i - 1] < 1.0)
        .collect();
    let loops = [24_000, 48_000, 60_000, 84_000, 108_000, 120_000, 144_000];
    assert_eq!(rises[..7], loops, "{rises:?}");
    assert_eq!(outcomes(&mut rig), [(id, Outcome::Applied { at: 0 })]);
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
    let watches = |rig: &Rig| rig.graph.requests.as_ref().unwrap().watches.len();
    assert_eq!(watches(&rig), 1);
    rig.live
        .edit(|change| {
            change.remove("clock");
            Ok(())
        })
        .unwrap();
    render_counted(&mut rig, 1);
    // It reported its first application, and ends without another outcome.
    let got = outcomes_of(&mut rig, id);
    assert!(matches!(got[..], [(_, Outcome::Applied { .. })]), "{got:?}");
    assert_eq!(watches(&rig), 0);
}

#[test]
fn a_grid_no_clock_reaches_is_refused() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    let value = RtValue::F32(0.5);
    // A repeating grid finer than a 256th note would split every block.
    let bad = [
        grid(0.0, 0.0, true),
        grid(-4.0, 0.0, false),
        grid(1.0 / 128.0, 0.0, true),
    ]
    .map(|spec| submit_on(&rig, ("dial", LEVEL), value, "clock", spec, None));
    render_counted(&mut rig, 1);
    let refused = Outcome::Refused(Refusal::Invalid);
    let expected = bad.map(|id| (id, refused));
    assert_eq!(outcomes(&mut rig), expected);
}

#[test]
fn a_repeating_grid_waits_and_applies_without_allocating() {
    let mut rig = beat_rig(22_500.0);
    rig.render(1);
    let id = submit_pulse(&rig, grid(1.0, 0.0, true));
    let out = render_counted(&mut rig, 20);
    // A beat every 128 samples, each on the sample ending them: one every
    // other block, never early.
    let applied = pulse_samples(&out, 64, 22_500.0, Vec::new());
    assert_eq!(applied, (0..10).map(|n| 128 * n + 127).collect::<Vec<_>>());
    let first = Outcome::Applied { at: applied[0] };
    assert_eq!(outcomes(&mut rig), [(id, first)]);
}
