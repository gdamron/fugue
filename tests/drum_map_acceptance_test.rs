//! Acceptance tests for the `drum_map` module, written from its specification
//! alone and driven through the public boundary: an invention document is
//! loaded into a `RenderEngine`, a clock's `beat_x4` steps the module, and one
//! output port at a time is rendered through a pass-through DAC.
//!
//! Layout of every rig: `clk` (120 BPM, 48 kHz => one 16th every 6000
//! samples) -> `drums.clock`; `drums.<port>` -> `dac` left. A second render of
//! the clock alone gives the sample positions of the clock's rising edges, so
//! "on the step grid" is checked against real edges, not computed ones.

use fugue::{ControlValue, RenderEngine};
use serde_json::{json, Value};

const SAMPLE_RATE: u32 = 48_000;
/// 120 BPM, a 16th note is a quarter of a beat: 0.125 s (the clock's edges
/// are within a sample of this spacing, so tests read the real edges).
const STEP: usize = 6_000;
const BAR: usize = 16 * STEP;
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
// Criterion 1: density 0 is silent, density max fills every allowed step
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Criterion 2: same x, y and seed reproduce the same pattern bit-for-bit
// ---------------------------------------------------------------------------

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
