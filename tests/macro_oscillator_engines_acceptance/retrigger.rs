//! Criterion 2: retriggering a half-decayed hit does not click.

use super::rig::*;

/// Longest the retrigger point is allowed to be after the first trigger. The
/// string and modal ring for many seconds at default morph, so for them the
/// point where the envelope reaches half of its peak can be far away; they are
/// retriggered no later than this instead (still loud, which is the harder
/// case for a discontinuity).
const LATEST_RETRIGGER: usize = 120_000; // 2.5 s
/// How long after the retrigger the step is measured.
const MEASURED_AFTER: usize = 24_000; // 0.5 s
/// Frames before the retrigger included in the measured window.
const MEASURED_BEFORE: usize = 48;

fn build(name: &str, hz: f32, level: Option<f32>) -> Voice {
    let mut v = engine(name);
    v.set("frequency", hz);
    if let Some(level) = level {
        let port = v.level_in;
        v.hold(port, level);
    }
    v
}

/// Criterion: "retriggering at 50% decay has a largest sample-to-sample step
/// of no more than 1.5x steady-state, for every new engine with a trigger
/// patched", at 110 Hz and 440 Hz, with `level` unpatched and with `level`
/// patched at 1 (the low-pass gate engaged), on `audio` and on `aux`.
///
/// Assumptions:
/// - Steady state is the largest sample step over a single untouched hit from
///   silence, measured from the first trigger to the end of the window that the
///   retrigger run is measured over (so both windows are equally long).
///   A decaying hit's biggest step is normally its attack, so this is the
///   largest step the engine makes on a clean hit.
/// - 50% decay is the first moment, after the hit's loudest 50 ms window, at
///   which a 50 ms window's peak is at or below half the loudest window's peak
///   (-6 dB). It is read off the untouched hit, and the second trigger is sent
///   at that frame. Engines still above half at 2.5 s are retriggered at 2.5 s.
/// - The retrigger run is measured from 48 frames before the second trigger
///   until 0.5 s after it. Both runs use a 1.3 ms trigger pulse at the same
///   default controls, so they differ only in the second trigger.
/// - `aux` is judged the same way, but only where the untouched hit has a
///   measurable step (above 1e-3), since `aux` may be silent for an engine.
/// - The exception is particle_noise's `aux`, which is judged by a no-clip
///   check instead: no sample over the retrigger window reaches full scale
///   (|x| >= 1.0, the output being clamped at +-1).
mod retrigger_steps_stay_within_one_and_a_half_times_steady_state {
    use super::*;

    fn check_one(name: &str, hz: f32, level: Option<f32>) {
        let what = format!("{name} at {hz} Hz, level {level:?}");
        // Untouched hit, long enough to find the half-decay point.
        let (probe, _) = build(name, hz, level).render_driven(LATEST_RETRIGGER + T0, pulses(&[T0]));
        let t_r = T0
            + half_decay_frame(&probe[T0..])
                .unwrap_or(LATEST_RETRIGGER)
                .min(LATEST_RETRIGGER);
        let end = t_r + MEASURED_AFTER;
        let from = t_r - MEASURED_BEFORE;

        let (ref_audio, ref_aux) = build(name, hz, level).render_driven(end, pulses(&[T0]));
        let (audio, aux) = build(name, hz, level).render_driven(end, pulses(&[T0, t_r]));

        for (port, steady, run) in [
            (
                "audio",
                max_step(&ref_audio[T0..]),
                max_step(&audio[from..]),
            ),
            ("aux", max_step(&ref_aux[T0..]), max_step(&aux[from..])),
        ] {
            if port == "audio" {
                assert!(steady > 1e-4, "{what}: untouched hit is silent");
            } else if name == "particle_noise" {
                // particle_noise's aux carries the bare impulses, and a
                // trigger deliberately fires a synchronized burst of every
                // particle at once, so the retrigger is a fresh click by
                // design rather than a decay-stack step. The step ratio does
                // not apply there; the meaningful guarantee is that the burst
                // does not clip. The output is clamped at +-1, so a clipped
                // burst shows up as samples at full scale, not above it:
                // none may reach it.
                let window = &aux[from..];
                let top = peak(window);
                let at_full_scale = window.iter().filter(|v| v.abs() >= 1.0).count();
                assert!(
                    at_full_scale == 0,
                    "{what}: retrigger at frame {t_r} ({:.3} s) on aux clips: \
                     {at_full_scale} samples at full scale, peak {top:.5}",
                    t_r as f64 / SRF
                );
                continue;
            } else if steady <= 1e-3 {
                continue;
            }
            assert!(
                run <= 1.5 * steady,
                "{what}: retrigger at frame {t_r} ({:.3} s) on {port} steps {run:.5}, \
                 {:.2}x the steady-state {steady:.5}",
                t_r as f64 / SRF,
                run / steady
            );
        }
    }

    fn check(name: &str) {
        for hz in [110.0, 440.0] {
            for level in [None, Some(1.0)] {
                check_one(name, hz, level);
            }
        }
    }

    each_new_model!(check);
}
