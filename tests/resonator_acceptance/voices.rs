//! Criterion 4: at polyphony 4 a new strum does not cut the previous tail.

use super::rig::*;
use super::*;

/// Analysis window for the energy at the first pitch.
const WINDOW: f64 = 0.1;
const FIRST: f64 = 220.0;
/// Later strums, each a fifth above the last, neither at a partial of the
/// first pitch's fundamental.
const LATER: [f32; 2] = [330.0, 495.0];

/// Criterion: "At polyphony 4, a new strum doesn't cut the previous tail":
/// strike a voice at 220 Hz, then (0.5 s apart) strum at 330 Hz and at
/// 495 Hz. Over the 100 ms Hann windows of `audio`, the magnitude at 220 Hz
/// keeps decaying the way it was decaying before the second strum: each
/// window after a new strum is at least half the window before it times the
/// previous window-to-window ratio (capped at 1), and at least a quarter of
/// the window before it outright. An abrupt cut would drop it to ~0.
/// Assumptions: the frequency control and the strum change in the same host
/// block, and the voice already ringing keeps its pitch (INTERFACE.md); the
/// damping is 0.9 so the tail is long and a cut is unmistakable; no upper
/// bound is asserted on the energy a later strum may add at 220 Hz.
#[test]
fn new_strum_does_not_cut_the_previous_tail() {
    let names = reso(json!({})).models;
    let window = seconds(WINDOW);
    for (position, model) in names.iter().enumerate() {
        let mut r = ready(position, FIRST as f32, 0.9);
        r.set("polyphony", 4.0);
        r.render(seconds(0.3));
        r.strike();
        let mut x = r.render_blocks(seconds(0.5), BLOCK);
        let mut strums = Vec::new();
        for hz in LATER {
            strums.push(x.len());
            r.set("frequency", hz);
            r.strike();
            x.extend(r.render_blocks(seconds(0.5), BLOCK));
        }
        let mag = |start: usize| dft_mag_hann(&x[start..start + window], FIRST);
        assert!(mag(seconds(0.2)) > 1e-3, "{model}: no energy at {FIRST} Hz");
        for strum in strums {
            // Windows tile the time before and after the strum.
            let (a, b) = (mag(strum - 2 * window), mag(strum - window));
            let decay = (b / a).min(1.0);
            let first_after = mag(strum);
            assert!(
                first_after >= 0.5 * b * decay && first_after >= 0.25 * b,
                "{model}: energy at {FIRST} Hz falls from {b} to {first_after} \
                 in the 100 ms after the strum at {:.2} s (earlier decay ratio {decay})",
                strum as f64 / SRF
            );
            let mut previous = first_after;
            for k in 1..4 {
                let current = mag(strum + k * window);
                assert!(
                    current >= 0.25 * previous,
                    "{model}: energy at {FIRST} Hz drops from {previous} to {current} \
                     {} ms after the strum at {:.2} s",
                    k * 100,
                    strum as f64 / SRF
                );
                previous = current;
            }
        }
    }
}

/// Criterion: the ringing voice stays audible to the end. After the second
/// strum the energy at the first pitch is still present half a second later
/// (it is the fundamental of a voice with damping 0.9) at no less than 5% of
/// its value just before the strum.
#[test]
fn first_voice_is_still_ringing_after_a_later_strum() {
    let names = reso(json!({})).models;
    let window = seconds(WINDOW);
    for (position, model) in names.iter().enumerate() {
        let mut r = ready(position, FIRST as f32, 0.9);
        r.set("polyphony", 4.0);
        r.render(seconds(0.3));
        r.strike();
        let mut x = r.render_blocks(seconds(0.5), BLOCK);
        let strum = x.len();
        r.set("frequency", 330.0);
        r.strike();
        x.extend(r.render_blocks(seconds(0.6), BLOCK));
        let before = dft_mag_hann(&x[strum - window..strum], FIRST);
        let later = dft_mag_hann(
            &x[strum + seconds(0.5)..strum + seconds(0.5) + window],
            FIRST,
        );
        assert!(
            later >= 0.05 * before,
            "{model}: energy at {FIRST} Hz is {later} 0.5 s after the strum, was {before}"
        );
    }
}
