//! Criterion 3: changing `model` or `polyphony` while ringing does not click.

use super::rig::*;
use super::*;

const HOST_BLOCK: usize = 128;
/// Steady-state ringing is measured over this long just before a change, and
/// the result over this long after it (at least the ~100 ms a crossfade takes).
const BEFORE: f64 = 0.1;
const AFTER: f64 = 0.15;
/// The bound is this multiple of the steady-state maximum step.
const RATIO: f32 = 1.5;
/// A steady maximum below this is treated as silence-level (1e-6 is -120 dBFS).
const FLOOR: f32 = 1e-6;

/// Renders `BEFORE`, applies `change`, renders `AFTER`, and requires the
/// largest step over the change and the 150 ms after (including the step
/// from the last old sample) to be at most `RATIO` times the largest step of
/// the steady ringing just before, on each of `audio`, `odd` and `even`.
/// Assumption: the change lands on a 128-frame host block boundary.
fn change_and_check(r: &mut Reso, what: &str, change: impl FnOnce(&mut Reso)) {
    let before = r.render_all(seconds(BEFORE), HOST_BLOCK);
    change(r);
    let after = r.render_all(seconds(AFTER), HOST_BLOCK);
    for ((name, b), (_, a)) in before.each().into_iter().zip(after.each()) {
        let bound = RATIO * max_step(b).max(FLOOR);
        let mut window = vec![*b.last().unwrap()];
        window.extend_from_slice(a);
        let worst = max_step(&window);
        assert!(
            worst <= bound,
            "{what}: {name} max step {worst} exceeds {RATIO} x steady max ({bound})"
        );
    }
}

fn struck(position: usize, hz: f32) -> Reso {
    let mut r = ready(position, hz, 0.75);
    r.strike();
    r.render_blocks(seconds(0.4), HOST_BLOCK);
    r
}

/// Criterion: "Changing `model` while ringing: the largest step is no more
/// than 1.5x steady-state", on `audio`, `odd` and `even`, between each
/// ordered pair of models present, struck at 440 Hz and at 110 Hz. With one
/// model present this passes vacuously.
/// Assumption: steady state is the 100 ms of ringing right before the
/// change, 0.4 s after the strike, at damping 0.75.
#[test]
fn switching_model_while_ringing_does_not_click() {
    let names = reso(json!({})).models;
    if names.len() < 2 {
        eprintln!("vacuous: only {} model present", names.len());
        return;
    }
    for a in 0..names.len() {
        for b in (0..names.len()).filter(|b| *b != a) {
            for hz in [440.0f32, 110.0] {
                let mut r = struck(a, hz);
                let what = format!("{} -> {} at {hz} Hz", names[a], names[b]);
                change_and_check(&mut r, &what, |r| r.select(b));
            }
        }
    }
}

/// Criterion: "Changing ... `polyphony` while ringing", 1 -> 2 -> 4 -> 1 in
/// turn on one struck voice, for every model, on `audio`, `odd` and `even`.
#[test]
fn changing_polyphony_while_ringing_does_not_click() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        let mut r = struck(position, 440.0);
        for polyphony in [2.0f32, 4.0, 1.0] {
            let what = format!("{model}: polyphony -> {polyphony}");
            change_and_check(&mut r, &what, |r| r.set("polyphony", polyphony));
        }
    }
}

/// Criterion: the same with several voices actually ringing: at polyphony 4,
/// four strums 100 ms apart (each at its own pitch), then 4 -> 2 -> 1 -> 4,
/// for every model.
/// Assumption: a voice keeps its pitch once the next strum moves on, so four
/// voices are sounding when the polyphony changes.
#[test]
fn changing_polyphony_with_several_voices_ringing_does_not_click() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        let mut r = ready(position, 220.0, 0.75);
        r.set("polyphony", 4.0);
        r.render(seconds(0.2));
        for hz in [220.0f32, 277.0, 330.0, 440.0] {
            r.set("frequency", hz);
            r.render_blocks(seconds(0.02), HOST_BLOCK);
            r.strike();
            r.render_blocks(seconds(0.08), HOST_BLOCK);
        }
        r.render_blocks(seconds(0.3), HOST_BLOCK);
        for polyphony in [2.0f32, 1.0, 4.0] {
            let what = format!("{model}: four voices ringing, polyphony -> {polyphony}");
            change_and_check(&mut r, &what, |r| r.set("polyphony", polyphony));
        }
    }
}
