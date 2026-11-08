//! Criterion 2: clock-locked ratio outputs stay phase-locked within +-1 sample
//! through a tempo change from 90 to 140 bpm. `looping`, `control` range,
//! `ratio` output mode, a gate into `clock`, tempo stepped mid-render.
//!
//! Two clocks drive it: the real `clock` module's `beat` output (tempo set
//! with its `bpm` control), and a test gate with whole-sample periods
//! (32000 then 20571, about 90 then 140 bpm). The real clock's periods are
//! fractional (20571.43), so the even-subdivision check (c) uses the test gate
//! only; checks (a) and (b) use both.
//!
//! Cycle starts are read from `slope` 0.98, where each ramp restarts at the
//! minimum after a short fall. Assumption: a start can be at any index >= 1
//! (sample 0 is never judged).

use super::rig::*;
use super::*;

const CHANGE: usize = 384_000; // 8 s, a multiple of 64
const FRAMES: usize = 864_000; // 18 s: 8 s at 90 bpm, 10 s at 140 bpm
const SLOW: usize = 32_000;
const FAST: usize = 20_571;

fn real_clock() -> Vec<f32> {
    let built = ModuleRegistry::default()
        .build("clock", SR, &json!({ "bpm": 90.0 }))
        .expect("clock builds");
    let mut clock = built.module;
    let beat = clock
        .module()
        .outputs()
        .iter()
        .position(|p| *p == "beat")
        .unwrap();
    let mut gate = Vec::with_capacity(FRAMES);
    for block in 0..FRAMES / 64 {
        if block * 64 == CHANGE {
            clock.module_mut().set_control("bpm", 140.0).unwrap();
        }
        assert!(clock.module_mut().process(64));
        gate.extend_from_slice(&clock.module().output_block(beat)[..64]);
    }
    gate
}

fn test_gate() -> Vec<f32> {
    (0..FRAMES)
        .map(|i| {
            let (since, period) = if i < CHANGE {
                (i, SLOW)
            } else {
                (i - CHANGE, FAST)
            };
            f32::from(since % period < period / 4)
        })
        .collect()
}

/// Renders the module on `shift` (0 = divisions, 1 = multiplications).
fn run(clock: &[f32], shift: f32) -> (Vec<usize>, Out) {
    let mut c = contour(
        json!({ "mode": "looping", "range": "control", "output_mode": "ratio",
                                "slope": 0.98, "frequency": 1.5, "shift": shift }),
    );
    c.block = 100;
    (rising(clock), c.render(FRAMES, &[("clock", clock)]))
}

/// Index into `edges` of the edge within 1 sample of `at`, if any.
fn edge_near(edges: &[usize], at: usize) -> Option<usize> {
    edges.iter().position(|e| e.abs_diff(at) <= 1)
}

fn check_every_edge_restarts_output_0(edges: &[usize], out: &Out, what: &str) {
    let starts = cycle_starts(&out[0]);
    for s in starts.iter().filter(|s| **s > edges[0]) {
        assert!(
            edge_near(edges, *s).is_some(),
            "{what}: cycle start {s} is off the clock"
        );
    }
    for e in &edges[1..edges.len() - 1] {
        assert!(
            starts.iter().any(|s| s.abs_diff(*e) <= 1),
            "{what}: no cycle start at clock edge {e}"
        );
    }
}

/// Output k at ratio 1/q starts only on edges, exactly q edges apart.
fn check_divisions(edges: &[usize], out: &Out, what: &str) {
    for (k, q) in [(1, 2), (2, 4), (3, 8)] {
        let on: Vec<usize> = cycle_starts(&out[k])
            .iter()
            .filter(|s| **s > edges[0])
            .map(|s| {
                edge_near(edges, *s)
                    .unwrap_or_else(|| panic!("{what}: ratio 1/{q} start {s} is off the clock"))
            })
            .collect();
        assert!(
            on.len() + 2 >= edges.len() / q,
            "{what}: ratio 1/{q} has {} starts",
            on.len()
        );
        for w in on.windows(2) {
            assert_eq!(w[1] - w[0], q, "{what}: ratio 1/{q} starts at edges {on:?}");
        }
    }
}

/// Output k at ratio n starts at every edge.
fn check_multiples_start_on_edges(edges: &[usize], out: &Out, what: &str) {
    for (k, channel) in out.iter().enumerate().skip(1) {
        let starts = cycle_starts(channel);
        for e in &edges[1..edges.len() - 1] {
            assert!(
                starts.iter().any(|s| s.abs_diff(*e) <= 1),
                "{what}: output {k} has no start at clock edge {e}"
            );
        }
    }
}

#[test]
fn output_0_restarts_on_every_clock_edge_through_the_tempo_change() {
    for (name, clock) in [("clock module", real_clock()), ("test gate", test_gate())] {
        for shift in [0.0, 1.0] {
            let (edges, out) = run(&clock, shift);
            assert!(edges.len() > 30, "{name}: {} edges", edges.len());
            check_every_edge_restarts_output_0(&edges, &out, &format!("{name} shift {shift}"));
        }
    }
}

#[test]
fn divided_outputs_start_on_every_qth_edge_through_the_tempo_change() {
    for (name, clock) in [("clock module", real_clock()), ("test gate", test_gate())] {
        let (edges, out) = run(&clock, 0.0);
        check_divisions(&edges, &out, &format!("{name} divisions"));
        let (edges, out) = run(&clock, 1.0);
        check_multiples_start_on_edges(&edges, &out, &format!("{name} multiples"));
    }
}

/// Interior starts of output n sit on the even subdivision of the clock
/// period, before the change (from the third edge) and from the second clock
/// period after it.
#[test]
fn multiplied_outputs_subdivide_the_clock_period_evenly() {
    let (edges, out) = run(&test_gate(), 1.0);
    let after = edges.iter().position(|e| *e >= CHANGE).unwrap();
    let periods: Vec<usize> = (2..after - 1).chain(after + 1..edges.len() - 1).collect();
    for (k, n) in [(1usize, 2usize), (2, 4), (3, 8)] {
        let starts = cycle_starts(&out[k]);
        for j in &periods {
            let (from, to) = (edges[*j], edges[*j + 1]);
            let inside: Vec<usize> = starts
                .iter()
                .copied()
                .filter(|s| *s > from + 1 && *s + 1 < to)
                .collect();
            assert_eq!(
                inside.len(),
                n - 1,
                "ratio {n}, period {from}..{to}: starts {inside:?}"
            );
            for (m, s) in inside.iter().enumerate() {
                let even = from as f64 + (m + 1) as f64 * (to - from) as f64 / n as f64;
                assert!(
                    (*s as f64 - even).abs() <= 1.0,
                    "ratio {n}, period {from}..{to}: start {s}, even subdivision {even:.2}"
                );
            }
        }
    }
}
