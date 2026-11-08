//! Acceptance tests for the `drum_map` module, written from its specification
//! alone and driven through the public boundary: an invention document is
//! loaded into a `RenderEngine`, a clock's `beat_x4` steps the module, and one
//! output port at a time is rendered through a pass-through DAC.
//!
//! Layout of every rig: `clk` (120 BPM, 48 kHz => one 16th every 6000
//! samples) -> `drums.clock`; `drums.<port>` -> `dac` left. A second render of
//! the clock alone gives the sample positions of the clock's rising edges, so
//! "on the step grid" is checked against real edges, not computed ones.

use fugue::{ControlValue, ModuleRegistry, RenderEngine};
use serde_json::{json, Value};

const SAMPLE_RATE: u32 = 48_000;
/// A trigger of 1.0 as a render writes it: the largest `f32` below 1.0,
/// where the device clamp stops.
const TRIGGER: f32 = 1.0 - f32::EPSILON / 2.0;
/// 120 BPM, a 16th note is a quarter of a beat: 0.125 s (the clock's edges
/// are within a sample of this spacing, so tests read the real edges).
const STEP: usize = 6_000;
const BAR: usize = 16 * STEP;
/// A render of exactly one bar: stops just before the clock edge that opens the next one.
const ONE_BAR: usize = BAR - 100;
const PORTS: [&str; 4] = ["kick", "snare", "hat", "accent"];
const VOICES: [&str; 3] = ["kick", "snare", "hat"];

type Edit = (usize, Vec<(&'static str, f32)>);

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn document(drum_config: &Value, source: &str, reset_clock: bool) -> String {
    let mut modules = vec![
        json!({ "id": "clk", "type": "clock", "config": { "bpm": 120.0 } }),
        json!({ "id": "dac", "type": "dac", "config": { "soft_clip": false } }),
    ];
    let mut connections = vec![];
    if source == "clock" {
        connections.push(json!({
            "from": "clk", "from_port": "beat_x4", "to": "dac", "to_port": "audio_left"
        }));
    } else {
        modules.push(json!({ "id": "drums", "type": "drum_map", "config": drum_config }));
        connections.push(json!({
            "from": "clk", "from_port": "beat_x4", "to": "drums", "to_port": "clock"
        }));
        connections.push(json!({
            "from": "drums", "from_port": source, "to": "dac", "to_port": "audio_left"
        }));
        if reset_clock {
            // A slow clock whose beat (every ~57600 samples = 9.6 steps)
            // never lands on a step edge after the first one.
            modules.push(json!({ "id": "rst", "type": "clock", "config": { "bpm": 50.0 } }));
            connections.push(json!({
                "from": "rst", "from_port": "beat", "to": "drums", "to_port": "reset"
            }));
        }
    }
    json!({ "version": "1.0.0", "modules": modules, "connections": connections }).to_string()
}

/// Renders the left channel of `source` ("clock" or a drum_map port) for
/// `frames` samples, applying each edit (position, [(control, value)]) as a
/// live control write between two render calls.
fn render(
    drum_config: &Value,
    source: &str,
    frames: usize,
    edits: &[Edit],
    reset_clock: bool,
) -> Vec<f32> {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine
        .load_json(&document(drum_config, source, reset_clock))
        .unwrap_or_else(|e| panic!("load failed: {e}"));
    let mut out = Vec::with_capacity(frames);
    let mut pos = 0;
    let mut schedule: Vec<&Edit> = edits.iter().collect();
    schedule.sort_by_key(|e| e.0);
    for edit in schedule.into_iter().map(Some).chain([None]) {
        let upto = edit.map(|e| e.0.min(frames)).unwrap_or(frames);
        if upto > pos {
            let mut buf = vec![0.0f32; (upto - pos) * 2];
            engine.render_interleaved(&mut buf).unwrap();
            out.extend(buf.chunks(2).map(|f| f[0]));
            pos = upto;
        }
        if let Some((_, controls)) = edit {
            for (key, value) in controls {
                engine
                    .set_control("drums", key, ControlValue::Number(*value))
                    .unwrap_or_else(|e| panic!("set_control {key}: {e}"));
            }
        }
    }
    out
}

/// Sample indices where the signal crosses 0.5 upward (a hit at sample 0
/// counts as a rise from silence).
fn rises(signal: &[f32]) -> Vec<usize> {
    let mut prev = 0.0f32;
    let mut out = vec![];
    for (i, &v) in signal.iter().enumerate() {
        if prev <= 0.5 && v > 0.5 {
            out.push(i);
        }
        prev = v;
    }
    out
}

/// Lengths of the high runs.
fn pulse_lengths(signal: &[f32]) -> Vec<usize> {
    let mut out = vec![];
    let mut run = 0;
    for &v in signal {
        if v > 0.5 {
            run += 1;
        } else if run > 0 {
            out.push(run);
            run = 0;
        }
    }
    out
}

fn clock_edges(frames: usize) -> Vec<usize> {
    rises(&render(&json!({}), "clock", frames, &[], false))
}

/// Rendered output of all four ports.
fn render_all(drum_config: &Value, frames: usize, edits: &[Edit]) -> Vec<Vec<f32>> {
    PORTS
        .iter()
        .map(|p| render(drum_config, p, frames, edits, false))
        .collect()
}

fn on_grid(rise: usize, edges: &[usize]) -> bool {
    edges.binary_search(&rise).is_ok()
}

fn total_hits(signals: &[Vec<f32>]) -> usize {
    signals.iter().map(|s| rises(s).len()).sum()
}

// ---------------------------------------------------------------------------
// Interface
// ---------------------------------------------------------------------------

/// Interface: module type `drum_map` has inputs `clock`/`reset`, outputs
/// `kick`/`snare`/`hat`/`accent`, and accepts every control as a config key.
#[test]
fn module_declares_its_ports_and_accepts_its_controls_as_config() {
    let registry = ModuleRegistry::default();
    assert!(registry.has_type("drum_map"), "drum_map is not registered");
    let inputs = registry.factory_input_ports("drum_map").unwrap_or(&[]);
    let outputs = registry.factory_output_ports("drum_map").unwrap_or(&[]);
    for p in ["clock", "reset"] {
        assert!(inputs.contains(&p), "missing input {p}: {inputs:?}");
    }
    for p in PORTS {
        assert!(outputs.contains(&p), "missing output {p}: {outputs:?}");
    }
    let config = json!({
        "x": 0.25, "y": 0.75, "kick_density": 0.4, "snare_density": 0.5,
        "hat_density": 0.6, "chaos": 0.1, "swing": 0.2, "seed": 3,
        "resolution": 16, "mode": "euclidean"
    });
    assert!(
        registry.build("drum_map", SAMPLE_RATE, &config).is_ok(),
        "all documented controls must be accepted as config keys"
    );
    assert!(registry
        .build("drum_map", SAMPLE_RATE, &json!({ "no_such_key": 1 }))
        .is_err());
}

/// Interface: outputs are triggers, 1.0 for a short pulse (about 5 ms), else
/// 0.0. A render writes 1.0 at the device clamp, just below it.
#[test]
fn triggers_are_short_pulses_of_one() {
    let cfg = json!({ "mode": "euclidean", "kick_density": 0.5 });
    let kick = render(&cfg, "kick", ONE_BAR, &[], false);
    assert!(
        kick.iter().all(|&v| v == 0.0 || v == TRIGGER),
        "not 0.0/1.0"
    );
    let lens = pulse_lengths(&kick);
    assert!(!lens.is_empty());
    let (lo, hi) = (
        SAMPLE_RATE as usize / 1000,
        SAMPLE_RATE as usize * 15 / 1000,
    );
    for len in lens {
        assert!(
            (lo..=hi).contains(&len),
            "pulse of {len} samples, want ~240"
        );
    }
}

// ---------------------------------------------------------------------------
// Criterion 1: density 0 is silent, density max fills every allowed step
// ---------------------------------------------------------------------------

/// Criterion 1 ("Density 0 is silent"): a voice at density 0 never fires,
/// whatever x, y, chaos and mode, and with every density 0 even accent is
/// silent.
#[test]
fn density_zero_never_fires() {
    let positions = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)];
    for mode in ["map", "euclidean"] {
        for (x, y) in positions {
            for chaos in [0.0, 1.0] {
                let cfg = json!({
                    "mode": mode, "x": x, "y": y, "chaos": chaos, "seed": 9,
                    "kick_density": 0.0, "snare_density": 0.0, "hat_density": 0.0
                });
                for port in PORTS {
                    let sig = render(&cfg, port, 2 * BAR, &[], false);
                    assert!(
                        rises(&sig).is_empty(),
                        "{port} fired at density 0 ({mode}, x={x}, y={y}, chaos={chaos})"
                    );
                }
            }
        }
    }
}

/// Criterion 1 ("density 0 is silent"), single voice: one voice at 0 stays
/// silent while the others play.
#[test]
fn density_zero_silences_only_that_voice() {
    for silent in VOICES {
        let mut cfg = json!({ "x": 0.3, "y": 0.8, "chaos": 0.5, "seed": 2 });
        for v in VOICES {
            cfg[format!("{v}_density")] = json!(if v == silent { 0.0 } else { 1.0 });
        }
        assert!(rises(&render(&cfg, silent, 2 * BAR, &[], false)).is_empty());
        let others: usize = VOICES
            .iter()
            .filter(|v| **v != silent)
            .map(|v| rises(&render(&cfg, v, 2 * BAR, &[], false)).len())
            .sum();
        assert!(others > 0, "other voices should still play");
    }
}

/// Criterion 1 ("density max fills every step the node allows"): at chaos 0,
/// raising density never removes a hit (subset chain), so density 1 holds
/// the largest hit set, and at every map position the largest set is
/// non-trivial for at least one position per voice.
#[test]
fn raising_density_only_adds_hits_and_max_is_largest() {
    let grid = [0.0, 0.5, 1.0];
    let densities = [0.0, 0.3, 0.6, 1.0];
    for voice in VOICES {
        let mut max_total = 0;
        for &x in &grid {
            for &y in &grid {
                let sets: Vec<Vec<usize>> = densities
                    .iter()
                    .map(|&d| {
                        let cfg = json!({
                            "x": x, "y": y, "chaos": 0.0,
                            "kick_density": d, "snare_density": d, "hat_density": d
                        });
                        rises(&render(&cfg, voice, BAR, &[], false))
                    })
                    .collect();
                for pair in sets.windows(2) {
                    assert!(
                        pair[0].iter().all(|h| pair[1].contains(h)),
                        "{voice} at ({x},{y}): hits {:?} not a subset of {:?}",
                        pair[0],
                        pair[1]
                    );
                }
                assert!(sets[0].is_empty());
                max_total += sets[3].len();
            }
        }
        assert!(
            max_total > 0,
            "{voice} never fires at density 1 anywhere on the map"
        );
    }
}

/// Criterion 1 ("density max fills every step the node allows"), euclidean
/// mode: density 1 is round(1 x 16) = 16 hits, every step of the bar, for
/// every voice, whatever x, y.
#[test]
fn euclidean_density_max_fills_every_step() {
    let edges = clock_edges(2 * BAR);
    for (x, y) in [(0.0, 0.0), (1.0, 1.0)] {
        let cfg = json!({
            "mode": "euclidean", "x": x, "y": y,
            "kick_density": 1.0, "snare_density": 1.0, "hat_density": 1.0
        });
        for voice in VOICES {
            let hits = rises(&render(&cfg, voice, 2 * BAR, &[], false));
            // The clock's 33rd edge may sit in the last sample of the render.
            assert!(
                (32..=33).contains(&hits.len()),
                "{voice}: {} hits",
                hits.len()
            );
            assert_eq!(
                hits[..32],
                edges[..32],
                "{voice} must fire on all 32 steps of 2 bars"
            );
        }
    }
}

/// Criterion 1 / interface ("Map position matters"): the four corners do not
/// all give the same pattern.
#[test]
fn map_corners_differ() {
    let patterns: Vec<Vec<Vec<usize>>> = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .iter()
        .map(|&(x, y)| {
            let cfg = json!({
                "x": x, "y": y, "chaos": 0.0,
                "kick_density": 1.0, "snare_density": 1.0, "hat_density": 1.0
            });
            VOICES
                .iter()
                .map(|v| rises(&render(&cfg, v, BAR, &[], false)))
                .collect()
        })
        .collect();
    assert!(
        patterns.iter().any(|p| *p != patterns[0]),
        "all four corners produced the identical pattern"
    );
}

// ---------------------------------------------------------------------------
// Criterion 2: same x, y and seed reproduce the same pattern bit-for-bit
// ---------------------------------------------------------------------------

/// Criterion 2 ("The same x, y and seed reproduce the same 1-bar pattern
/// bit-for-bit"): two independent renders agree sample for sample on all four
/// outputs over one bar and over four bars, with chaos above 0.
#[test]
fn same_x_y_seed_reproduce_the_pattern_bit_for_bit() {
    for (x, y, seed, chaos) in [
        (0.2, 0.7, 7, 0.6),
        (0.9, 0.1, 123, 1.0),
        (0.5, 0.5, 0, 0.25),
    ] {
        let cfg = json!({
            "x": x, "y": y, "seed": seed, "chaos": chaos,
            "kick_density": 0.8, "snare_density": 0.7, "hat_density": 0.9
        });
        for frames in [BAR, 4 * BAR] {
            let a = render_all(&cfg, frames, &[]);
            let b = render_all(&cfg, frames, &[]);
            assert!(total_hits(&a) > 0, "the pattern should not be empty");
            for (port, (sa, sb)) in PORTS.iter().zip(a.iter().zip(&b)) {
                assert_eq!(
                    sa.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    sb.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    "{port} differs between renders (x={x}, y={y}, seed={seed}, chaos={chaos}, {frames} frames)"
                );
            }
        }
    }
}

/// Criterion 2, chaos 0: the seed only picks the chaos variation, so with
/// chaos 0 two different seeds give the identical pattern. (Inferred from
/// "seed picks the chaos variation".)
#[test]
fn seed_is_irrelevant_without_chaos() {
    let make = |seed: u32| {
        let cfg = json!({
            "x": 0.4, "y": 0.6, "seed": seed, "chaos": 0.0,
            "kick_density": 0.7, "snare_density": 0.7, "hat_density": 0.7
        });
        render_all(&cfg, 2 * BAR, &[])
    };
    assert_eq!(make(1), make(99));
}

// ---------------------------------------------------------------------------
// Criterion 3: moving x/y mid-bar keeps every trigger on the step grid
// ---------------------------------------------------------------------------

/// Criterion 3 ("Moving x/y mid-bar keeps every trigger on the step grid"):
/// x and y are written live at sample offsets that fall inside steps and
/// bars, with swing 0 and resolution 16; every rising edge of every output
/// lands on a sample where the clock rises.
#[test]
fn moving_x_y_mid_bar_keeps_triggers_on_the_clock_grid() {
    let frames = 4 * BAR;
    let edges = clock_edges(frames);
    let edits: Vec<Edit> = vec![
        (2_500, vec![("x", 0.9), ("y", 0.1)]),
        (14_001, vec![("x", 0.1), ("y", 0.9)]),
        (31_111, vec![("x", 1.0)]),
        (50_000, vec![("y", 0.0)]),
        (77_777, vec![("x", 0.0), ("y", 1.0)]),
        (100_003, vec![("x", 0.33), ("y", 0.66)]),
        (181_999, vec![("x", 0.77)]),
        (250_001, vec![("y", 0.5), ("x", 0.5)]),
        (300_300, vec![("x", 0.2), ("y", 0.8)]),
    ];
    for chaos in [0.0, 0.5] {
        let cfg = json!({
            "x": 0.5, "y": 0.5, "chaos": chaos, "seed": 4, "swing": 0.0, "resolution": 16,
            "kick_density": 1.0, "snare_density": 1.0, "hat_density": 1.0
        });
        let signals = render_all(&cfg, frames, &edits);
        assert!(total_hits(&signals) > 0, "expected some hits");
        for (port, sig) in PORTS.iter().zip(&signals) {
            for r in rises(sig) {
                assert!(
                    on_grid(r, &edges),
                    "{port} rose at sample {r}, which is not a clock edge (chaos {chaos})"
                );
            }
        }
    }
}

/// Criterion 3, moving density and chaos live as well as x/y: still on the
/// grid.
#[test]
fn live_control_moves_of_any_pattern_control_stay_on_the_grid() {
    let frames = 2 * BAR;
    let edges = clock_edges(frames);
    let edits: Vec<Edit> = vec![
        (4_321, vec![("kick_density", 0.2), ("chaos", 0.8)]),
        (
            45_678,
            vec![("snare_density", 0.1), ("hat_density", 1.0), ("seed", 5.0)],
        ),
        (91_234, vec![("x", 0.1), ("y", 0.2), ("chaos", 0.0)]),
    ];
    let cfg = json!({ "kick_density": 1.0, "snare_density": 1.0, "hat_density": 0.5 });
    for (port, sig) in PORTS.iter().zip(render_all(&cfg, frames, &edits)) {
        for r in rises(&sig) {
            assert!(on_grid(r, &edges), "{port} rose off-grid at {r}");
        }
    }
}

/// Interface (straight timing): with swing 0 every hit of every output in
/// the static case rises exactly on a clock edge, and the first clock edge
/// plays step 0 (euclidean step 0 is always hit when a voice has hits).
#[test]
fn first_clock_edge_plays_step_zero_on_the_grid() {
    let edges = clock_edges(BAR);
    let cfg = json!({
        "mode": "euclidean",
        "kick_density": 0.25, "snare_density": 0.5, "hat_density": 0.75
    });
    for voice in VOICES {
        let hits = rises(&render(&cfg, voice, BAR, &[], false));
        assert_eq!(hits.first(), Some(&edges[0]), "{voice}: step 0 missing");
        assert!(hits.iter().all(|h| on_grid(*h, &edges)));
    }
}

// ---------------------------------------------------------------------------
// Accent
// ---------------------------------------------------------------------------

/// Interface ("accent pulses together with a hit ... never alone"): every
/// accent rise coincides with a kick, snare or hat rise.
#[test]
fn accent_only_pulses_together_with_a_voice_hit() {
    for mode in ["map", "euclidean"] {
        let cfg = json!({
            "mode": mode, "x": 0.6, "y": 0.4, "chaos": 0.3, "seed": 8,
            "kick_density": 0.7, "snare_density": 0.6, "hat_density": 0.9
        });
        let signals = render_all(&cfg, 4 * BAR, &[]);
        let voice_hits: Vec<usize> = signals[..3].iter().flat_map(|s| rises(s)).collect();
        for r in rises(&signals[3]) {
            assert!(voice_hits.contains(&r), "accent alone at {r} ({mode})");
        }
    }
}

// ---------------------------------------------------------------------------
// Euclidean mode
// ---------------------------------------------------------------------------

/// Interface (Euclidean mode): a voice with density d places round(d x 16)
/// hits evenly over the bar, step 0 included whenever it has hits. "Evenly"
/// is checked as every cyclic gap between hits being floor or ceil of
/// 16 / hits.
#[test]
fn euclidean_places_round_density_times_16_hits_evenly() {
    let edges = clock_edges(3 * BAR);
    for d in [0.0, 0.0625, 0.2, 0.3125, 0.5, 0.6, 0.75, 0.9, 1.0] {
        let expected = (d * 16.0f32).round() as usize;
        for (x, y) in [(0.5, 0.5), (0.0, 1.0)] {
            let cfg = json!({
                "mode": "euclidean", "x": x, "y": y,
                "kick_density": d, "snare_density": d, "hat_density": d
            });
            for voice in VOICES {
                let hits = rises(&render(&cfg, voice, 3 * BAR, &[], false));
                for bar in 0..3 {
                    let steps: Vec<usize> = hits
                        .iter()
                        .map(|h| edges.binary_search(h).expect("off-grid hit"))
                        .filter(|s| s / 16 == bar)
                        .map(|s| s % 16)
                        .collect();
                    assert_eq!(steps.len(), expected, "{voice} d={d} bar {bar}: {steps:?}");
                    if expected == 0 {
                        continue;
                    }
                    assert_eq!(steps[0], 0, "{voice} d={d}: step 0 must be included");
                    let (lo, hi) = (16 / expected, 16usize.div_ceil(expected));
                    for i in 0..expected {
                        let next = if i + 1 < expected { steps[i + 1] } else { 16 };
                        let gap = next - steps[i];
                        assert!(
                            gap == lo || gap == hi,
                            "{voice} d={d}: uneven gap {gap} in {steps:?}"
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reset
// ---------------------------------------------------------------------------

/// Interface (`reset`): a rising edge makes the next clock edge step 0 of a
/// new bar. A second clock rises at about sample 57600 (during step 9); the
/// next clock edge (edge 10) must play step 0 again (euclidean d=0.25 hits
/// steps 0, 4, 8, 12), and the second reset at about 115200 restarts the bar
/// at edge 20. Without the reset input the same window plays a different
/// phase.
#[test]
fn reset_makes_the_next_clock_edge_step_zero() {
    let cfg = json!({ "mode": "euclidean", "kick_density": 0.25 });
    let frames = 2 * BAR;
    let edges = clock_edges(frames);
    let within = |hits: Vec<usize>| -> Vec<usize> {
        hits.into_iter()
            .filter(|h| (edges[9]..edges[20]).contains(h))
            .collect()
    };
    let with_reset = within(rises(&render(&cfg, "kick", frames, &[], true)));
    assert_eq!(with_reset, vec![edges[10], edges[14], edges[18]]);
    let without = within(rises(&render(&cfg, "kick", frames, &[], false)));
    assert_ne!(
        with_reset, without,
        "reset must change the phase of the bar"
    );
}

// ---------------------------------------------------------------------------
// Swing and resolution
// ---------------------------------------------------------------------------

/// Interface (`swing`): odd steps are delayed by swing x half a step, even
/// steps stay on the clock edge. Euclidean density 1 fires every step.
#[test]
fn swing_delays_only_odd_steps_by_swing_times_half_a_step() {
    let edges = clock_edges(BAR + STEP);
    for swing in [0.5f32, 1.0] {
        let cfg = json!({ "mode": "euclidean", "kick_density": 1.0, "swing": swing });
        let hits = rises(&render(&cfg, "kick", ONE_BAR, &[], false));
        assert_eq!(hits.len(), 16);
        let delay = (swing * STEP as f32 / 2.0) as i64;
        for (step, hit) in hits.iter().enumerate() {
            let want = edges[step] as i64 + if step % 2 == 1 { delay } else { 0 };
            assert!(
                (*hit as i64 - want).abs() <= 3,
                "swing {swing} step {step}: rose at {hit}, expected about {want}"
            );
        }
    }
}

/// Interface (`resolution`): at 32 an extra step falls half-way between clock
/// edges; every hit is on an edge or on a half-step, and both occur. (How
/// euclidean mode scales its hit count at 32 is not specified; only that
/// density 1 fires more than 16 times per bar is asserted.)
#[test]
fn resolution_32_adds_a_step_half_way_between_clock_edges() {
    let edges = clock_edges(2 * BAR);
    let cfg = json!({
        "mode": "euclidean", "kick_density": 1.0, "resolution": 32, "swing": 0.0
    });
    let hits = rises(&render(&cfg, "kick", ONE_BAR, &[], false));
    assert!(
        hits.len() > 16,
        "expected more than 16 hits, got {}",
        hits.len()
    );
    let (mut on_edge, mut on_half) = (0, 0);
    for h in hits {
        let i = edges.partition_point(|e| *e <= h + 2) - 1;
        let half = edges[i] + (edges[i + 1] - edges[i]) / 2;
        if (h as i64 - edges[i] as i64).abs() <= 2 {
            on_edge += 1;
        } else if (h as i64 - half as i64).abs() <= 2 {
            on_half += 1;
        } else {
            panic!("hit at {h} is neither on an edge nor half-way");
        }
    }
    assert!(on_edge > 0 && on_half > 0, "edge {on_edge}, half {on_half}");
}

// ---------------------------------------------------------------------------
// Criterion 4 (optional): generator script header lists sources and licences
// ---------------------------------------------------------------------------

/// Criterion 4 (sources recorded): the repository carries a generator script
/// for the pattern data under `scripts/` with `drum_map` in its name, whose
/// header comment names its sources and licences.
#[test]
fn generator_script_header_lists_sources_and_licences() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
    let script = std::fs::read_dir(&dir)
        .expect("scripts/ exists")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("drum_map"))
        })
        .expect("a scripts/*drum_map* generator script");
    let text = std::fs::read_to_string(&script).unwrap();
    let header: String = text
        .lines()
        .take(40)
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    assert!(header.contains("source"), "header must list sources");
    assert!(header.contains("licen"), "header must list licences");
}
