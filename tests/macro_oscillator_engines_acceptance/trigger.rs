//! Trigger semantics: a patched trigger that never fires stays quiet, and an
//! unpatched one sounds continuously.

use super::rig::*;

const HZ: f32 = 110.0;

fn build(name: &str) -> Voice {
    let mut v = engine(name);
    v.set("frequency", HZ);
    v
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
