//! The shape of the glide when a cable goes, read directly: a constant
//! through a vca whose `level` input is the sum of several cables, so the
//! output is the sum itself (clamped to 0..1) and every sample can be
//! checked. The control behind the input is 0.25, where the input rests once
//! no cable is left.

use super::*;

const REST: f32 = 0.25;
const EPS: f32 = 1e-5;

/// The ways to drop a cable `b` (a module `b` feeds `amp.level`).
const DISCONNECT: Edit = Edit::Disconnect(Edge("b", "audio", "amp", "level"));

/// A constant 1.0 through `amp`, whose level is the sum of `levels`' cables.
/// Each source is a vca named after its letter.
fn document_of(levels: &[(&'static str, f64)]) -> Value {
    let mut modules = vec![
        constant("tone"),
        module("amp", "vca", json!({ "level": f64::from(REST) })),
        dac(),
    ];
    let mut connections = vec![wire("tone", "amp", "audio"), wire("amp", "dac", "audio")];
    for &(id, level) in levels {
        let source = constant_source(id, level);
        modules.extend(source.modules);
        connections.extend(source.wires);
        connections.push(wire(id, "amp", "level"));
    }
    document(modules, connections)
}

fn disconnect(id: &'static str) -> Edit {
    Edit::Disconnect(Edge(id, "audio", "amp", "level"))
}

/// The largest |y[i] - y[i-1]| over `from..to`, counting `from`'s predecessor.
fn largest_step(y: &[f32], from: usize, to: usize) -> f32 {
    max_step(y, from, to)
}

/// Every sample in `from..to` lies within `[low, high]` and none rises.
fn only_falls_between(
    y: &[f32],
    from: usize,
    to: usize,
    low: f32,
    high: f32,
) -> Result<(), String> {
    for i in from..to {
        if !(low - EPS..=high + EPS).contains(&y[i]) {
            return Err(format!("frame {i}: {} outside {low}..{high}", y[i]));
        }
        if i > from && y[i] > y[i - 1] + 1e-6 {
            return Err(format!("frame {i}: rises from {} to {}", y[i - 1], y[i]));
        }
    }
    Ok(())
}

fn equals(y: &[f32], from: usize, to: usize, want: f32) -> Result<(), String> {
    let off = max_difference(y, &vec![want; y.len()], from, to);
    if off <= EPS {
        Ok(())
    } else {
        Err(format!("{off} from {want} over frames {from}..{to}"))
    }
}

/// Criterion: removing one of several summed cables, by disconnect, by
/// removing its source or by a live reload, glides from the old sum to the
/// new one over the 10 ms release, then holds the remaining sum exactly.
#[test]
fn a_removed_cable_glides_over_the_release_to_the_remaining_sum() {
    let both = document_of(&[("a", 0.5), ("b", 0.25)]);
    let one = document_of(&[("a", 0.5)]);
    let mut cases = Cases::default();
    for (mode, how, edit) in [
        (Mode::Offline, "disconnect", DISCONNECT),
        (Mode::Offline, "remove source", Edit::Remove("b")),
        (Mode::Live, "disconnect", DISCONNECT),
        (Mode::Live, "remove source", Edit::Remove("b")),
        (Mode::Live, "reload", Edit::Reload(one.clone())),
    ] {
        let label = format!("{mode:?} / {how}");
        let mut rig = Rig::start(mode, &both);
        let mut y = rig.render(BEFORE);
        rig.apply(&edit);
        y.extend(rig.render(AFTER));
        cases.check(
            format!("{label} (the sum plays before)"),
            equals(&y, 256, BEFORE, 0.75),
        );
        cases.check(
            format!("{label} (falls monotonically, never past the new sum)"),
            only_falls_between(&y, BEFORE - 1, BEFORE + AFTER, 0.5, 0.75),
        );
        let even = 0.25 / RELEASE as f32;
        let step = largest_step(&y, BEFORE, BEFORE + AFTER);
        cases.check(
            format!("{label} (steps no more than 1.5x an even glide)"),
            if step <= even * 1.5 {
                Ok(())
            } else {
                Err(format!("largest step {step} past an even glide's {even}"))
            },
        );
        cases.check(
            format!("{label} (mid-release, between old and new)"),
            if (0.55..=0.70).contains(&y[BEFORE + RELEASE / 2]) {
                Ok(())
            } else {
                Err(format!(
                    "{} half a release after the edit",
                    y[BEFORE + RELEASE / 2]
                ))
            },
        );
        cases.check(
            format!("{label} (exactly the remaining sum after the release)"),
            equals(&y, BEFORE + RELEASE + BLOCK, BEFORE + AFTER, 0.5),
        );
    }
    cases.finish();
}

/// Criterion: a cable removed early in the note (four blocks in) glides
/// just the same.
#[test]
fn a_cable_removed_right_after_the_note_starts_still_glides() {
    let both = document_of(&[("a", 0.5), ("b", 0.25)]);
    let mut cases = Cases::default();
    let start = 4 * BLOCK;
    for mode in [Mode::Offline, Mode::Live] {
        let mut rig = Rig::start(mode, &both);
        let mut y = rig.render(start);
        rig.apply(&DISCONNECT);
        y.extend(rig.render(AFTER));
        let step = largest_step(&y, start, start + AFTER);
        cases.check(
            format!("{mode:?}"),
            only_falls_between(&y, start + BLOCK, start + AFTER, 0.5, 0.75)
                .and(equals(&y, start + RELEASE + BLOCK, start + AFTER, 0.5))
                .and(if step <= 1.5 * 0.25 / RELEASE as f32 {
                    Ok(())
                } else {
                    Err(format!("largest step {step}"))
                }),
        );
    }
    cases.finish();
}

/// Criterion: a cable patched back in during the glide returns the sum; no
/// sample leaves the old-sum..new-sum range or goes non-finite.
#[test]
fn a_cable_patched_back_during_the_glide_returns_the_sum() {
    let both = document_of(&[("a", 0.5), ("b", 0.25)]);
    let mut cases = Cases::default();
    for mode in [Mode::Offline, Mode::Live] {
        let mut rig = Rig::start(mode, &both);
        let mut y = rig.render(BEFORE);
        rig.apply(&DISCONNECT);
        y.extend(rig.render(2 * BLOCK));
        let step = largest_step(&y, BEFORE, y.len());
        cases.check(
            format!("{mode:?} (the cable's removal does not step)"),
            if step <= 1.5 * 0.25 / RELEASE as f32 && y[y.len() - 1] < 0.75 - 1e-3 {
                Ok(())
            } else {
                Err(format!("largest step {step}, or the glide never started"))
            },
        );
        rig.apply(&Edit::Connect(Edge("b", "audio", "amp", "level")));
        y.extend(rig.render(AFTER));
        cases.check(
            format!("{mode:?} (stays between the sums, finite)"),
            if y.iter().all(|v| v.is_finite())
                && y[BEFORE..]
                    .iter()
                    .all(|v| (0.5 - EPS..=0.75 + EPS).contains(v))
            {
                Ok(())
            } else {
                Err("left the range of the two sums".to_string())
            },
        );
        cases.check(
            format!("{mode:?} (the sum plays again)"),
            equals(&y, BEFORE + 2 * RELEASE, y.len(), 0.75),
        );
    }
    cases.finish();
}

/// Criterion: several cables removed one after another, the second while
/// the first still glides, each glide ends at the right sum and the input
/// never steps or rises.
#[test]
fn two_cables_removed_one_after_another_glide_to_the_remaining_sum() {
    let all = document_of(&[("a", 0.5), ("b", 0.25), ("c", 0.125)]);
    let mut cases = Cases::default();
    for mode in [Mode::Offline, Mode::Live] {
        // The second goes 128 frames into the first's glide.
        let mut rig = Rig::start(mode, &all);
        let mut y = rig.render(BEFORE);
        rig.apply(&disconnect("b"));
        y.extend(rig.render(2 * BLOCK));
        rig.apply(&disconnect("c"));
        y.extend(rig.render(AFTER));
        let label = format!("{mode:?} / overlapping");
        cases.check(
            format!("{label} (falls to the last cable alone)"),
            only_falls_between(&y, BEFORE - 1, BEFORE + 2 * BLOCK + AFTER, 0.5, 0.875),
        );
        let step = largest_step(&y, BEFORE, y.len());
        let limit = 1.5 * (0.25 + 0.125) / RELEASE as f32;
        cases.check(
            format!("{label} (no step)"),
            if step <= limit {
                Ok(())
            } else {
                Err(format!("largest step {step} past {limit}"))
            },
        );
        cases.check(
            format!("{label} (settles on the remaining cable)"),
            equals(&y, BEFORE + 2 * BLOCK + RELEASE + BLOCK, y.len(), 0.5),
        );

        // The second goes after the first has settled.
        let mut rig = Rig::start(mode, &all);
        let mut y = rig.render(BEFORE);
        rig.apply(&disconnect("b"));
        y.extend(rig.render(AFTER));
        let second_at = y.len();
        rig.apply(&disconnect("c"));
        y.extend(rig.render(AFTER));
        let label = format!("{mode:?} / one after the other");
        cases.check(
            format!("{label} (first settles on 0.625)"),
            equals(&y, BEFORE + RELEASE + BLOCK, second_at, 0.625),
        );
        cases.check(
            format!("{label} (second glides to 0.5)"),
            only_falls_between(&y, second_at - 1, y.len(), 0.5, 0.625).and(equals(
                &y,
                second_at + RELEASE + BLOCK,
                y.len(),
                0.5,
            )),
        );
        let step = largest_step(&y, second_at, y.len());
        cases.check(
            format!("{label} (second does not step)"),
            if step <= 1.5 * 0.125 / RELEASE as f32 {
                Ok(())
            } else {
                Err(format!("largest step {step}"))
            },
        );
    }
    cases.finish();
}

/// Criterion: removing one cable and then the last one still glides: the
/// final full unpatch goes from the remaining cable's value to the
/// control's, without a step, whether the first glide has finished or not.
#[test]
fn removing_one_cable_then_the_last_one_still_glides_without_a_step() {
    let both = document_of(&[("a", 0.5), ("b", 0.25)]);
    let mut cases = Cases::default();
    for mode in [Mode::Offline, Mode::Live] {
        // The last goes after the first has settled.
        let mut rig = Rig::start(mode, &both);
        let mut y = rig.render(BEFORE);
        rig.apply(&disconnect("b"));
        y.extend(rig.render(AFTER));
        let last_at = y.len();
        rig.apply(&disconnect("a"));
        y.extend(rig.render(AFTER));
        let label = format!("{mode:?} / after the first settled");
        cases.check(
            format!("{label} (first glides from 0.75 to 0.5)"),
            only_falls_between(&y, BEFORE - 1, last_at, 0.5, 0.75).and(equals(
                &y,
                BEFORE + RELEASE + BLOCK,
                last_at,
                0.5,
            )),
        );
        let first = largest_step(&y, BEFORE, last_at);
        cases.check(
            format!("{label} (first does not step)"),
            if first <= 1.5 * 0.25 / RELEASE as f32 {
                Ok(())
            } else {
                Err(format!("largest step {first}"))
            },
        );
        cases.check(
            format!("{label} (last glides from 0.5 to rest)"),
            only_falls_between(&y, last_at - 1, y.len(), REST, 0.5).and(equals(
                &y,
                last_at + RELEASE + BLOCK,
                y.len(),
                REST,
            )),
        );
        let step = largest_step(&y, last_at, y.len());
        let even = 0.25 / RELEASE as f32;
        cases.check(
            format!("{label} (last does not step)"),
            if step <= 1.5 * even {
                Ok(())
            } else {
                Err(format!("largest step {step} past an even glide's {even}"))
            },
        );

        // The last goes while the first still glides.
        let mut rig = Rig::start(mode, &both);
        let mut y = rig.render(BEFORE);
        rig.apply(&disconnect("b"));
        y.extend(rig.render(2 * BLOCK));
        rig.apply(&disconnect("a"));
        y.extend(rig.render(AFTER));
        let label = format!("{mode:?} / during the first glide");
        cases.check(
            format!("{label} (falls to rest)"),
            only_falls_between(&y, BEFORE - 1, y.len(), REST, 0.75).and(equals(
                &y,
                BEFORE + 2 * BLOCK + RELEASE + BLOCK,
                y.len(),
                REST,
            )),
        );
        let step = largest_step(&y, BEFORE, y.len());
        let limit = 1.5 * 0.75 / RELEASE as f32;
        cases.check(
            format!("{label} (no step)"),
            if step <= limit {
                Ok(())
            } else {
                Err(format!("largest step {step} past {limit}"))
            },
        );
    }
    cases.finish();
}
