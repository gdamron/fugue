//! Trigger semantics: a rising edge above 0.3 fires, the input must fall
//! below 0.1 to re-arm, and a patched trigger that never fires stays quiet.

use super::rig::*;

const HZ: f32 = 110.0;
/// Longest the second trigger waits, as in the retrigger test.
const LATEST: usize = 120_000;
const WINDOW: usize = 2_880; // 60 ms

fn build(name: &str) -> Voice {
    let mut v = engine(name);
    v.set("frequency", HZ);
    v
}

/// Criterion: "a rising edge above 0.3 [fires]; it must fall below 0.1 to
/// re-arm". A hit is started, then a second rise to 1 happens at the hit's
/// half-decay point (defined as in the retrigger test) in two ways: from 0
/// (re-armed, the control) and from 0.2, which is above the 0.1 re-arm level
/// (not re-armed). Three renders: no second rise (U), a rise from 0 (C) and a
/// rise from 0.2 (H).
/// Assumptions: a retrigger is visible as a higher peak in the 60 ms after the
/// second rise. If C does not exceed U by 1.3x (an engine whose retrigger does
/// not show up in the peak at its half-decay point) the engine cannot be
/// judged this way and the test returns; otherwise H must be within 1.15x of U,
/// i.e. no new hit.
mod a_trigger_must_fall_below_a_tenth_to_rearm {
    use super::*;

    fn check(name: &str) {
        let (probe, _) = build(name).render_driven(LATEST + T0, pulses(&[T0]));
        let t_r = T0 + half_decay_frame(&probe[T0..]).unwrap_or(LATEST).min(LATEST);
        let frames = t_r + WINDOW;
        let after = |x: &[f32]| peak(&x[t_r..t_r + WINDOW]);

        let (undisturbed, _) = build(name).render_driven(frames, pulses(&[T0]));
        let (rearmed, _) = build(name).render_driven(frames, pulses(&[T0, t_r]));
        let (above_rearm, _) = build(name).render_driven(frames, move |i| {
            if i >= t_r || (T0..T0 + PULSE).contains(&i) {
                1.0
            } else if i >= T0 {
                0.2
            } else {
                0.0
            }
        });
        let (u, c, h) = (after(&undisturbed), after(&rearmed), after(&above_rearm));
        if c < 1.3 * u {
            return;
        }
        assert!(
            h <= 1.15 * u,
            "{name}: a rise from 0.2 gave peak {h} after frame {t_r}; undisturbed {u}, re-armed {c}"
        );
    }

    each_new_model!(check);
}

/// Criterion: "a rising edge above 0.3": a trigger input that sits at 0.25
/// never crosses the threshold, so a patched-but-unfired trigger produces far
/// less than a real hit.
/// Assumption: a patched trigger that never fires leaves the engine without
/// excitation, so the peak over one second is under a tenth of a triggered
/// hit's peak.
mod a_trigger_below_the_threshold_does_not_fire {
    use super::*;

    fn check(name: &str) {
        let (hit, _) = build(name).render_driven(seconds(1.0), pulses(&[T0]));
        let (quiet, _) = build(name).render_driven(seconds(1.0), |_| 0.25);
        assert!(peak(&hit) > 0.01, "{name}: the control hit is silent");
        assert!(
            peak(&quiet) < 0.1 * peak(&hit),
            "{name}: a 0.25 trigger gave peak {}, a real hit {}",
            peak(&quiet),
            peak(&hit)
        );
    }

    each_new_model!(check);
}

/// Criterion: "with no trigger patched, engines are un-triggered (continuous
/// excitation, they sound continuously)".
/// Assumption: sounding continuously means every 0.5 s window from 0.5 s to
/// 3.0 s has a peak above 0.003 (about -50 dBFS) at default controls.
mod an_unpatched_trigger_sounds_continuously {
    use super::*;

    fn check(name: &str) {
        let mut v = build(name);
        let x = v.render(seconds(3.0));
        let windows = windowed_peaks(&x[seconds(0.5)..], seconds(0.5), seconds(0.5));
        assert_eq!(windows.len(), 5);
        for (k, p) in windows.iter().enumerate() {
            assert!(*p > 0.003, "{name}: window {k} after 0.5 s has peak {p}");
        }
    }

    each_new_model!(check);
}
