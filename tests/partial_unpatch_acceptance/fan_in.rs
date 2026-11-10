//! One of two cables summed into an input goes mid-note. Each target is a
//! held tone whose level, loudness, timbre or pitch is set by the sum of two
//! cables; the edit lands at a block boundary right at a sine peak, so a
//! step in the sum shows as a step many times the tone's own steepest step.
//!
//! The same cases run against every target, so nothing here can pass by
//! handling one module type.

use super::*;

/// What "no step" means for a target.
#[derive(Clone, Copy)]
enum Measure {
    /// The largest sample-to-sample step in the 20 ms after the edit is at
    /// most this many times the largest in the 20 ms before it.
    Steps(f32),
    /// A pitch input: the tone has no sample step to measure, so no cycle
    /// may differ from the one before it by more than this many samples
    /// (an even glide from 30 to 60 samples a cycle changes one by about 7;
    /// a step by 24), at least four cycles pass at lengths between the old
    /// and the new, and 15 ms on, cycles last `settled` samples.
    Cycles { jump: f64, settled: f64 },
}

struct Target {
    name: &'static str,
    build: fn(Cables) -> Value,
    /// The cable that goes.
    edge: Edge,
    /// The module feeding that cable, removed to drop it.
    source: &'static str,
    measure: Measure,
    /// Whether 15 ms after the edit the output matches, sample by sample,
    /// the document that never had the cable.
    comparable: bool,
    /// Whether the removed cable's contribution is large enough that the
    /// output must still sound it (at least twice the remaining level) a few
    /// milliseconds into the release: a filter's output is continuous
    /// through a cutoff jump, so a step shows only as the sound dying away
    /// in under a millisecond instead of over the release.
    lingers: bool,
}

/// The step limit full-unpatch tests use for a modulation input.
const MODULATION: Measure = Measure::Steps(1.5);
/// ... and for an audio input.
const AUDIO: Measure = Measure::Steps(3.0);

/// A sine through a vca into the dac; the vca's `level` is the sum of
/// cables of 0.5 each (1.0 for both, 0.5 after).
fn vca_level(cables: Cables) -> Value {
    fan_in(
        (
            vec![
                tone("tone", json!({})),
                module("amp", "vca", json!({ "level": 0.25 })),
                dac(),
            ],
            vec![wire("tone", "amp", "audio"), wire("amp", "dac", "audio")],
        ),
        &constant_source("a", 0.5),
        &constant_source("b", 0.5),
        ("amp", "level"),
        cables,
    )
}

/// A sine with full amplitude modulation depth: the scale is (sum + 1) / 2,
/// 1.0 for the two cables (0.2 and 0.8) and 0.6 after.
fn amplitude_mod(cables: Cables) -> Value {
    fan_in(
        (
            vec![tone("tone", json!({ "amplitude_mod_depth": 1.0 })), dac()],
            vec![wire("tone", "dac", "audio")],
        ),
        &constant_source("a", 0.2),
        &constant_source("b", 0.8),
        ("tone", "amplitude_mod"),
        cables,
    )
}

/// A 1687.5 Hz sine through a low-pass at 500 Hz; the sum of 0.01 and 0.99
/// opens it fully, and 0.01 alone leaves it at about 700 Hz.
fn cutoff_mod(cables: Cables) -> Value {
    fan_in(
        (
            vec![
                tone("tone", json!({ "frequency": 1687.5 })),
                module(
                    "flt",
                    "filter",
                    json!({ "filter_type": "lowpass", "cutoff": 500.0,
                            "resonance": 0.0, "cutoff_mod_depth": 20000.0 }),
                ),
                dac(),
            ],
            vec![wire("tone", "flt", "audio"), wire("flt", "dac", "audio")],
        ),
        &constant_source("a", 0.01),
        &constant_source("b", 0.99),
        ("flt", "cutoff_mod"),
        cables,
    )
}

/// A sine whose `frequency` input is the sum of two cables of 800 Hz: 1600
/// Hz (30 samples a cycle) for both, 800 Hz (60 samples) after.
fn frequency(cables: Cables) -> Value {
    fan_in(
        (
            vec![tone("tone", json!({})), dac()],
            vec![wire("tone", "dac", "audio")],
        ),
        &thousand_source("a", 0.8),
        &thousand_source("b", 0.8),
        ("tone", "frequency"),
        cables,
    )
}

/// Two sines of amplitude 0.4 summed in the dac: 0.8 for both, 0.4 after.
fn dac_audio(cables: Cables) -> Value {
    fan_in(
        (vec![dac()], vec![]),
        &tone_source("a", 0.4),
        &tone_source("b", 0.4),
        ("dac", "audio"),
        cables,
    )
}

/// Two sines of amplitude 0.4 summed in a vca's audio input at level 1.
fn vca_audio(cables: Cables) -> Value {
    fan_in(
        (
            vec![module("amp", "vca", json!({ "level": 1.0 })), dac()],
            vec![wire("amp", "dac", "audio")],
        ),
        &tone_source("a", 0.4),
        &tone_source("b", 0.4),
        ("amp", "audio"),
        cables,
    )
}

fn targets() -> Vec<Target> {
    let target = |name, build, to: (&'static str, &'static str), measure, comparable| Target {
        name,
        build,
        edge: Edge("b", "audio", to.0, to.1),
        source: "b",
        measure,
        comparable,
        lingers: false,
    };
    vec![
        target("vca level", vca_level, ("amp", "level"), MODULATION, true),
        target(
            "oscillator amplitude_mod",
            amplitude_mod,
            ("tone", "amplitude_mod"),
            MODULATION,
            true,
        ),
        Target {
            lingers: true,
            ..target(
                "filter cutoff_mod",
                cutoff_mod,
                ("flt", "cutoff_mod"),
                MODULATION,
                true,
            )
        },
        target(
            "oscillator frequency",
            frequency,
            ("tone", "frequency"),
            Measure::Cycles {
                jump: 16.0,
                settled: 60.0,
            },
            false,
        ),
        target("dac audio", dac_audio, ("dac", "audio"), AUDIO, true),
        target("vca audio", vca_audio, ("amp", "audio"), AUDIO, true),
    ]
}

/// Renders the two-cable document, makes `edit` at `BEFORE`, and renders on.
fn play(t: &Target, mode: Mode, edit: &Edit) -> Vec<f32> {
    let mut rig = Rig::start(mode, &(t.build)(Cables::Both));
    let mut out = rig.render(BEFORE);
    rig.apply(edit);
    out.extend(rig.render(AFTER));
    out
}

/// Positions of the upward zero crossings in `from..to`, interpolated.
fn upward_crossings(y: &[f32], from: usize, to: usize) -> Vec<f64> {
    (from.max(1)..to.min(y.len()))
        .filter(|&i| y[i - 1] < 0.0 && y[i] >= 0.0)
        .map(|i| i as f64 - 1.0 + f64::from(-y[i - 1]) / f64::from(y[i] - y[i - 1]))
        .collect()
}

fn no_step(y: &[f32], measure: Measure) -> Result<(), String> {
    match measure {
        Measure::Steps(limit) => steps_within(y, BEFORE, limit),
        Measure::Cycles { jump, .. } => {
            let crossings = upward_crossings(y, BEFORE - 20 * MS, BEFORE + AFTER);
            let cycles: Vec<f64> = crossings.windows(2).map(|p| p[1] - p[0]).collect();
            let worst = cycles
                .windows(2)
                .map(|p| (p[1] - p[0]).abs())
                .fold(0.0, f64::max);
            if cycles.len() < 10 {
                return Err(format!("only {} cycles to measure", cycles.len()));
            }
            let between = cycles.iter().filter(|c| (36.0..=54.0).contains(*c)).count();
            if between < 4 {
                return Err(format!(
                    "only {between} cycles between the old and new pitch: the pitch stepped"
                ));
            }
            if worst > jump {
                return Err(format!(
                    "a cycle differs by {worst:.1} samples from the one before \
                     (limit {jump}): the pitch stepped"
                ));
            }
            Ok(())
        }
    }
}

/// The output has become what the remaining cable alone gives.
fn settled(t: &Target, y: &[f32], remaining: &[f32]) -> Result<(), String> {
    if t.comparable {
        let off = max_difference(y, remaining, BEFORE + 15 * MS, BEFORE + AFTER);
        if off <= 0.01 {
            Ok(())
        } else {
            Err(format!(
                "still {off:.4} from the remaining sound after 15 ms"
            ))
        }
    } else {
        let Measure::Cycles { settled, .. } = t.measure else {
            unreachable!("only a pitch target is not comparable");
        };
        let crossings = upward_crossings(y, BEFORE + 15 * MS, BEFORE + AFTER);
        if crossings.len() < 4 {
            return Err(format!("only {} cycles to measure", crossings.len()));
        }
        for pair in crossings.windows(2) {
            let cycle = pair[1] - pair[0];
            if (cycle - settled).abs() > 0.75 {
                return Err(format!(
                    "a cycle at {:.0} lasts {cycle:.2} samples, not the remaining {settled}",
                    pair[0]
                ));
            }
        }
        Ok(())
    }
}

/// 3 to 6 ms into the release the removed cable still opens the filter: the
/// output is at least twice as loud as it ends up.
fn lingers(y: &[f32]) -> Result<(), String> {
    let during = max_abs(&y[BEFORE + 3 * MS..BEFORE + 6 * MS]);
    let after = max_abs(&y[BEFORE + AFTER - 5 * MS..BEFORE + AFTER]);
    if during >= 2.0 * after {
        Ok(())
    } else {
        Err(format!(
            "{during:.3} peak 3 to 6 ms in, only {:.1}x the final {after:.3}",
            during / after
        ))
    }
}

fn run(ways: impl Fn(&Target) -> Vec<(Mode, &'static str, Edit)>) {
    let all = targets();
    assert!(
        all.len() >= 3,
        "the criteria hold for at least three module types"
    );
    let mut cases = Cases::default();
    for t in &all {
        // The same tone with the second cable never made.
        let remaining = render_offline(&(t.build)(Cables::WithoutCable), BEFORE + AFTER);
        for (mode, how, edit) in ways(t) {
            let label = format!("{} / {mode:?} / {how}", t.name);
            let y = play(t, mode, &edit);
            cases.check(format!("{label} (no step)"), no_step(&y, t.measure));
            cases.check(
                format!("{label} (settles on the remaining cable)"),
                settled(t, &y, &remaining),
            );
            if t.lingers {
                cases.check(
                    format!("{label} (the cutoff glides rather than jumps)"),
                    lingers(&y),
                );
            }
        }
    }
    cases.finish();
}

/// Criterion: two cables are summed into one input during a sustained note,
/// and removing one of them (a disconnect of that one connection) produces
/// no step. Criterion: this holds for at least three module types.
#[test]
fn disconnecting_one_of_two_summed_cables_does_not_step() {
    run(|t| {
        vec![
            (Mode::Offline, "disconnect", Edit::Disconnect(t.edge)),
            (Mode::Live, "disconnect", Edit::Disconnect(t.edge)),
        ]
    });
}

/// Criterion: removing one route of a fan-in during a sustained note
/// produces no step, whether the source module goes (offline and live) or a
/// live reload drops the connection, or the source with it. Criterion: this
/// holds for at least three module types.
#[test]
fn removing_one_route_of_a_fan_in_does_not_step() {
    run(|t| {
        vec![
            (Mode::Offline, "remove source", Edit::Remove(t.source)),
            (Mode::Live, "remove source", Edit::Remove(t.source)),
            (
                Mode::Live,
                "reload without the connection",
                Edit::Reload((t.build)(Cables::WithoutCable)),
            ),
            (
                Mode::Live,
                "reload without the source",
                Edit::Reload((t.build)(Cables::WithoutSource)),
            ),
        ]
    });
}
