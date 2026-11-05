//! Criterion 4: a mode or range change has a largest sample-to-sample step
//! of at most 1.5x the largest steady-state step of either setting. Control
//! range at 1 and 4 Hz, audio range at 100 and 1000 Hz (at very low LFO
//! rates a bounded crossfade can't meet a slope-relative bound).
//!
//! `contour.0` in the default `phase` output mode, `slope` 0.5, driven by a
//! trigger/gate train at the module's own frequency so every mode has
//! something to do. The change lands at an arbitrary (non-block) sample and
//! the window runs from the last sample before it to 2 cycles (control) or
//! 150 ms (audio) past the change. The steady-state reference is each
//! setting alone over the same drive, after one settling cycle.

use super::rig::*;
use super::*;

/// Output 0 over `frames`, switching `key` to `to` at sample `at`.
fn render(
    base: &Value,
    key: &str,
    from: &str,
    to: Option<(usize, &str)>,
    freq: f32,
    frames: usize,
) -> Vec<f32> {
    let period = (SRF / f64::from(freq)).round() as usize;
    let drive = pulse_train(frames, period, period / 2);
    let mut config = base.clone();
    config[key] = json!(from);
    config["frequency"] = json!(freq);
    let mut c = contour(config);
    c.block = 128;
    let Some((at, name)) = to else {
        return c.render(frames, &[("trigger", &drive[..])])[0].clone();
    };
    let mut x = c.render(at, &[("trigger", &drive[..])])[0].clone();
    c.choose(key, name);
    x.extend(c.render(frames - at, &[("trigger", &drive[..])])[0].iter());
    x
}

fn check(base: Value, key: &str, a: &str, b: &str, freq: f32, window: usize) {
    let period = (SRF / f64::from(freq)).round() as usize;
    let (settle, at) = (period, 2 * period + 777);
    let frames = at + window;
    let steady = |name: &str| max_step(&render(&base, key, name, None, freq, frames)[settle..]);
    let bound = 1.5 * steady(a).max(steady(b));
    let x = render(&base, key, a, Some((at, b)), freq, frames);
    let worst = max_step(&x[at - 1..]);
    assert!(
        worst <= bound,
        "{key} {a} -> {b} at {freq} Hz: step {worst} exceeds 1.5 x steady ({bound})"
    );
}

#[test]
fn control_range_mode_changes_do_not_step() {
    let base = json!({ "range": "control", "slope": 0.5 });
    let modes = ["looping", "ad", "ar"];
    for freq in [1.0, 4.0] {
        let period = (SRF / f64::from(freq)) as usize;
        for a in modes {
            for b in modes.iter().filter(|b| **b != a) {
                check(base.clone(), "mode", a, b, freq, 2 * period + 5000);
            }
        }
    }
}

#[test]
fn audio_range_changes_do_not_step() {
    let base = json!({ "mode": "looping", "slope": 0.5 });
    for freq in [100.0, 1000.0] {
        let window = (0.15 * SRF) as usize;
        check(base.clone(), "range", "control", "audio", freq, window);
        check(base.clone(), "range", "audio", "control", freq, window);
        for (a, b) in [("looping", "ad"), ("looping", "ar"), ("ar", "looping")] {
            let audio = json!({ "range": "audio", "slope": 0.5 });
            check(audio, "mode", a, b, freq, window);
        }
    }
}
