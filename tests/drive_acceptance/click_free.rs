//! Definition of done: "changes are click-free" -- shape changes and drive
//! sweeps must not click.
//!
//! The bound: a sample-to-sample step in the output of a changing setting
//! must not exceed what the setting's own steady-state output already does.
//! The steady-state largest step of each shape at each drive involved is
//! measured on a fresh instance with a 220 Hz, -6 dBFS sine (it is large for
//! `bits` and `rate`, which are stepped by nature). While a shape crossfades,
//! each step is a blend of the two shapes' steps plus the crossfade's own
//! change, at most 2 / 960 samples (full swing over 20 ms), so 1.25x the
//! larger steady step plus 0.01 is a bound a clean change meets with margin
//! while a click (a jump of a good part of the signal's swing) fails it.

use super::rig::*;
use super::*;

const AMP: f64 = 0.5;
const F: f64 = 220.0;

fn steady_step(shape: &str, drive: f64) -> f32 {
    let mut rig = Rig::new(json!({ "shape": shape, "drive": drive, "mix": 1.0, "level": 1.0 }));
    let s = sine(AMP, F);
    rig.run(secs(0.5), &s);
    max_step(&rig.run(secs(0.2), &s))
}

fn bound(steps: impl IntoIterator<Item = f32>) -> f32 {
    1.25 * steps.into_iter().fold(0.0, f32::max) + 0.01
}

/// Every ordered pair of shapes, switched live (numeric index) at drive 0.75
/// in the middle of a settled signal.
#[test]
fn switching_shape_live_does_not_click() {
    let steady: Vec<f32> = SHAPES.iter().map(|s| steady_step(s, 0.75)).collect();
    let s = sine(AMP, F);
    for (a, from) in SHAPES.iter().enumerate() {
        for (b, to) in SHAPES.iter().enumerate().filter(|(b, _)| *b != a) {
            let mut rig = Rig::new(json!({ "shape": from, "drive": 0.75, "mix": 1.0 }));
            rig.block = 64;
            let mut out = rig.run(secs(0.3), &s)[secs(0.25)..].to_vec();
            rig.set("shape", b as f32);
            out.extend(rig.run(secs(0.4), &s));
            let (step, limit) = (max_step(&out), bound([steady[a], steady[b]]));
            assert!(step <= limit, "{from} -> {to}: step {step} > {limit}");
        }
    }
}

/// Cycling through all shapes every 50 ms for 1.2 s stays click-free.
#[test]
fn cycling_shapes_quickly_does_not_click() {
    let steady = SHAPES.iter().map(|s| steady_step(s, 0.75));
    let limit = bound(steady);
    let s = sine(AMP, F);
    let mut rig = Rig::new(json!({ "shape": "soft", "drive": 0.75, "mix": 1.0 }));
    rig.block = 64;
    rig.run(secs(0.3), &s);
    let out = rig.run_with(secs(1.2), &s, &mut |t, m| {
        if t % secs(0.05) < 64 {
            m.set_control("shape", ((t / secs(0.05)) % 6) as f32)
                .unwrap();
        }
    });
    let step = max_step(&out);
    assert!(step <= limit, "step {step} > {limit}");
}

/// Drive swept 0 -> 1 -> 0 live in small steps (one per 64-frame block, over
/// 1 s each way) for every shape.
#[test]
fn sweeping_drive_live_does_not_click() {
    let s = sine(AMP, F);
    for shape in SHAPES {
        let limit = bound([0.0, 0.25, 0.5, 0.75, 1.0].map(|d| steady_step(shape, d)));
        let mut rig = Rig::new(json!({ "shape": shape, "drive": 0.0, "mix": 1.0 }));
        rig.block = 64;
        rig.run(secs(0.5), &s);
        let sweep = secs(1.0);
        let t0 = rig.t;
        let out = rig.run_with(2 * sweep, &s, &mut |t, m| {
            let p = (t - t0) as f32 / sweep as f32;
            m.set_control("drive", if p < 1.0 { p } else { 2.0 - p })
                .unwrap();
        });
        let step = max_step(&out);
        assert!(step <= limit, "{shape} drive sweep: step {step} > {limit}");
    }
}
