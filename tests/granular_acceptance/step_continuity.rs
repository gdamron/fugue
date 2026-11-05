//! Criterion: "Freeze, unfreeze and mode change each have a largest step of
//! no more than 1.5x steady-state."
//!
//! Method: a smooth two-sine input. The steady-state step is the larger of
//! the largest step in a one second window before the event and in a one
//! second window after it has settled (same mode and controls within each
//! window). The event window runs from the sample before the event to 250 ms
//! after it, and must stay within 1.5x that steady-state step on both
//! channels.
//!
//! Assumption: where the spec gives no steady-state for a window (for example
//! an unfrozen `looping_delay` that has not been tapped), the other window
//! supplies it; the larger of the two is the bound, and it must not be zero.

use super::rig::*;
use super::*;

const EVENT: usize = 192_000; // 4 s, on a block boundary
const CONFIG_BUFFER: f64 = 2.0;

fn steps(channel: &[f32], from: usize, to: usize) -> f32 {
    max_step(&channel[from..to])
}

/// Renders with `start` controls and `events`, then checks the event at
/// `EVENT` against the steady-state steps on both channels.
fn check_event(label: &str, start: &[(&str, f32)], events: &[Event]) {
    let feed = Feed::mono(smooth(EVENT + s(2.6)));
    let config = json!({ "buffer_length": CONFIG_BUFFER, "seed": 3 });
    let (l, r) = render_granular(config, start, events, &feed);
    let (pre, post) = (EVENT - s(1.0)..EVENT, EVENT + s(1.5)..EVENT + s(2.5));
    let window = EVENT - 1..EVENT + s(0.25);
    for (name, ch) in [("left", &l), ("right", &r)] {
        let steady = steps(ch, pre.start, pre.end).max(steps(ch, post.start, post.end));
        let around = steps(ch, window.start, window.end);
        assert!(steady > 1e-5, "{label} {name}: steady state is silent");
        assert!(
            around <= 1.5 * steady,
            "{label} {name}: step {around} around the event vs steady-state {steady} (x{:.2})",
            around / steady
        );
    }
}

/// Freeze on, in each mode: the input keeps moving while the buffer stops.
#[test]
fn freeze_on_has_no_click_in_every_mode() {
    for (name, mode) in MODES {
        check_event(
            &format!("freeze on in {name}"),
            &[("mode", mode)],
            &[(EVENT, "freeze", 1.0)],
        );
    }
}

/// Freeze off, in each mode: frozen for two seconds, then recording resumes.
#[test]
fn freeze_off_has_no_click_in_every_mode() {
    for (name, mode) in MODES {
        check_event(
            &format!("freeze off in {name}"),
            &[("mode", mode)],
            &[(EVENT - s(2.0), "freeze", 1.0), (EVENT, "freeze", 0.0)],
        );
    }
}

/// Mode change, between every ordered pair of modes.
#[test]
fn every_mode_change_has_no_click() {
    for (from, a) in MODES {
        for (to, b) in MODES {
            if from == to {
                continue;
            }
            check_event(
                &format!("mode change {from} -> {to}"),
                &[("mode", a)],
                &[(EVENT, "mode", b)],
            );
        }
    }
}

/// Mode change while frozen (freeze and mode combine without a click).
#[test]
fn mode_change_while_frozen_has_no_click() {
    for (from, a) in MODES {
        for (to, b) in MODES {
            if from == to {
                continue;
            }
            check_event(
                &format!("frozen mode change {from} -> {to}"),
                &[("mode", a)],
                &[(EVENT - s(2.0), "freeze", 1.0), (EVENT, "mode", b)],
            );
        }
    }
}
