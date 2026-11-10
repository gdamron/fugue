//! Disconnecting a connection into a modulation input mid-note: the
//! parameter moves smoothly to its resting value (what the input reads
//! unconnected: 0 for a plain modulation input, the control's value for an
//! input that overrides a control when patched).
//!
//! Each target is a held 187.5 Hz sine whose level, loudness or timbre is
//! driven by a constant 1.0 until the connection is dropped. The edit lands
//! at a block boundary right at a sine peak, so an unsmoothed change shows
//! as a step many times the sine's own steepest step.

use super::*;

/// One module wired so that a constant source drives a modulation input.
struct Target {
    name: &'static str,
    /// The document with (`true`) or without the connection into the input.
    build: fn(connected: bool) -> Value,
    edge: Edge,
    /// The module feeding the input, removed to drop the connection.
    source: &'static str,
    /// Whether the output can be compared sample by sample against the
    /// document that never had the connection.
    comparable: bool,
}

fn wiring(mut connections: Vec<Value>, edge: Edge, connected: bool) -> Vec<Value> {
    if connected {
        connections.push(edge.json());
    }
    connections
}

pub const VCA_LEVEL: Edge = Edge("src", "audio", "amp", "level");

/// A sine through a vca whose level control is 0.25: a constant 1.0 patched
/// into `level` plays it at full level, and unpatched it rests at 0.25.
pub fn vca_level(connected: bool) -> Value {
    document(
        vec![
            tone("tone", json!({})),
            constant("src"),
            module("amp", "vca", json!({ "level": 0.25 })),
            dac(),
        ],
        wiring(
            vec![wire("tone", "amp", "audio"), wire("amp", "dac", "audio")],
            VCA_LEVEL,
            connected,
        ),
    )
}

pub const AMPLITUDE_MOD: Edge = Edge("src", "audio", "tone", "amplitude_mod");

/// A sine with full amplitude modulation depth: a constant 1.0 on
/// `amplitude_mod` plays it at full scale, and unpatched (0) at half.
pub fn amplitude_mod(connected: bool) -> Value {
    document(
        vec![
            tone("tone", json!({ "amplitude_mod_depth": 1.0 })),
            constant("src"),
            dac(),
        ],
        wiring(vec![wire("tone", "dac", "audio")], AMPLITUDE_MOD, connected),
    )
}

pub const CUTOFF_MOD: Edge = Edge("src", "audio", "flt", "cutoff_mod");

/// A sine through a low-pass at 500 Hz whose `cutoff_mod` of 1.0 opens it
/// fully (20 kHz), and unpatched (0) leaves it at 500 Hz.
///
/// The resting cutoff sits at 500 Hz so the filter's own settling stays
/// well inside the 15 ms the parameter has to arrive in: a pole at 500 Hz
/// has a time constant of 1/(2*pi*500) = 0.32 ms, so five of them are
/// 1.6 ms. (At 100 Hz they are 8 ms, and the test would measure the
/// filter's ringing down, not the cutoff.) The tone is 1687.5 Hz, 3.4x the
/// resting cutoff, so it is clearly attenuated at rest (about 22 dB for the
/// 2nd-order low-pass, 10.7 dB for a single pole) and a jump in the cutoff
/// would show. 1687.5 Hz is 9 cycles per 256 samples, so its peaks still
/// fall on the sample where the edit lands.
pub fn cutoff_mod(connected: bool) -> Value {
    document(
        vec![
            tone("tone", json!({ "frequency": 1687.5 })),
            constant("src"),
            module(
                "flt",
                "filter",
                json!({ "filter_type": "lowpass", "cutoff": 500.0,
                        "resonance": 0.0, "cutoff_mod_depth": 20000.0 }),
            ),
            dac(),
        ],
        wiring(
            vec![wire("tone", "flt", "audio"), wire("flt", "dac", "audio")],
            CUTOFF_MOD,
            connected,
        ),
    )
}

pub const FREQUENCY: Edge = Edge("g3", "audio", "tone", "frequency");

/// A sine whose `frequency` input is patched from a constant 1000 Hz (three
/// +20 dB gains on a constant 1.0); unpatched it rests at its control, 187.5.
pub fn frequency(connected: bool) -> Value {
    let gain = |id: &str| module(id, "gain", json!({ "decibels": 20.0 }));
    document(
        vec![
            tone("tone", json!({})),
            constant("src"),
            gain("g1"),
            gain("g2"),
            gain("g3"),
            dac(),
        ],
        wiring(
            vec![
                wire("src", "g1", "audio"),
                wire("g1", "g2", "audio"),
                wire("g2", "g3", "audio"),
                wire("tone", "dac", "audio"),
            ],
            FREQUENCY,
            connected,
        ),
    )
}

fn targets() -> Vec<Target> {
    vec![
        Target {
            name: "vca level",
            build: vca_level,
            edge: VCA_LEVEL,
            source: "src",
            comparable: true,
        },
        Target {
            name: "oscillator amplitude_mod",
            build: amplitude_mod,
            edge: AMPLITUDE_MOD,
            source: "src",
            comparable: true,
        },
        Target {
            name: "filter cutoff_mod",
            build: cutoff_mod,
            edge: CUTOFF_MOD,
            source: "src",
            comparable: true,
        },
        Target {
            name: "oscillator frequency",
            build: frequency,
            edge: FREQUENCY,
            source: "g3",
            comparable: false,
        },
    ]
}

/// Every way to drop the connection, offline and live.
fn ways(t: &Target) -> Vec<(Mode, &'static str, Edit)> {
    vec![
        (Mode::Offline, "disconnect", Edit::Disconnect(t.edge)),
        (Mode::Offline, "remove source", Edit::Remove(t.source)),
        (Mode::Live, "disconnect", Edit::Disconnect(t.edge)),
        (Mode::Live, "remove source", Edit::Remove(t.source)),
        (Mode::Live, "reload", Edit::Reload((t.build)(false))),
    ]
}

/// Renders the connected document, makes `edit` at `BEFORE`, and renders on.
fn play(t: &Target, mode: Mode, edit: &Edit) -> Vec<f32> {
    let mut rig = Rig::start(mode, &(t.build)(true));
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

/// Once 15 ms have passed the tone cycles at the control's frequency.
fn rests_at_the_control_frequency(y: &[f32]) -> Result<(), String> {
    let crossings = upward_crossings(y, BEFORE + 15 * MS, BEFORE + AFTER);
    if crossings.len() < 4 {
        return Err(format!("only {} cycles to measure", crossings.len()));
    }
    for pair in crossings.windows(2) {
        let cycle = pair[1] - pair[0];
        if (cycle - 256.0).abs() > 0.75 {
            return Err(format!(
                "a cycle at {:.0} lasts {cycle:.2} samples, not the resting 256",
                pair[0]
            ));
        }
    }
    Ok(())
}

#[test]
fn a_dropped_modulation_connection_moves_the_parameter_smoothly() {
    let mut cases = Cases::default();
    for t in targets() {
        // The same tone with the connection never made: the resting sound.
        let resting = render_offline(&(t.build)(false), BEFORE + AFTER);
        for (mode, how, edit) in ways(&t) {
            let label = format!("{} / {mode:?} / {how}", t.name);
            let y = play(&t, mode, &edit);
            cases.check(
                format!("{label} (step within 1.5x steady)"),
                steps_within(&y, BEFORE, 1.5),
            );
            let settled = if t.comparable {
                let off = max_difference(&y, &resting, BEFORE + 15 * MS, BEFORE + AFTER);
                if off <= 0.01 {
                    Ok(())
                } else {
                    Err(format!("still {off:.4} from the resting sound after 15 ms"))
                }
            } else {
                rests_at_the_control_frequency(&y)
            };
            cases.check(format!("{label} (rests within 15 ms)"), settled);
        }
    }
    cases.finish();
}
