//! A patched `level` at 0 silences every new engine.

use super::rig::*;

/// -80 dBFS as a peak amplitude.
const FLOOR: f32 = 1e-4;

/// A trigger every 0.25 s.
fn train(frames: usize) -> Vec<usize> {
    (0..)
        .map(|k| T0 + k * seconds(0.25))
        .take_while(|t| *t < frames)
        .collect()
}

/// Criterion: "`level` patched at 0 gives silence (below -80 dBFS) after it
/// settles", on `audio`, both un-triggered and with a trigger every 0.25 s,
/// from the start (50 ms allowed) and when the level drops from 1 to 0 while
/// the engine is sounding (0.5 s allowed, as the gate closes, with `decay` set to
/// 0.1 s so a 60 dB fall takes 0.1 s). A control
/// first shows that the same drive at level 1 is audible (peak above 0.01), so
/// the silence is the level's doing and not a dead engine.
/// Assumption: a trigger at level 0 does not lift the gate.
mod level_input_at_zero_is_silent {
    use super::*;

    fn silent(name: &str, what: &str, x: &[f32], allowed: f64) {
        let p = peak(&x[seconds(allowed)..]);
        assert!(p < FLOOR, "{name} ({what}): peak {p:e} is above -80 dBFS");
    }

    fn check(name: &str) {
        let frames = seconds(1.0);
        for triggered in [false, true] {
            let drive = if triggered {
                "triggered"
            } else {
                "un-triggered"
            };
            let pulse = pulses(&train(seconds(2.0)));
            let render = |v: &mut Voice, n: usize| {
                if triggered {
                    v.render_driven(n, &pulse).0
                } else {
                    v.render(n)
                }
            };

            let mut loud = engine(name);
            let port = loud.level_in;
            loud.hold(port, 1.0);
            let p = peak(&render(&mut loud, frames));
            assert!(
                p > 0.01,
                "{name} ({drive}): peak {p} at level 1 is not audible"
            );

            let mut v = engine(name);
            let port = v.level_in;
            v.hold(port, 0.0);
            silent(
                name,
                &format!("{drive}, level 0 from the start"),
                &render(&mut v, frames),
                0.05,
            );

            let mut v = engine(name);
            v.set("decay", 0.1);
            v.hold(port, 1.0);
            render(&mut v, seconds(0.5));
            v.hold(port, 0.0);
            silent(
                name,
                &format!("{drive}, level dropped to 0"),
                &render(&mut v, frames),
                0.5,
            );
        }
    }

    each_new_model!(check);
}
