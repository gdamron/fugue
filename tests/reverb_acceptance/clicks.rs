//! Criterion 3: room_size, decay and freeze changes are click-free.

use super::rig::*;

struct Event {
    at: f64,
    key: &'static str,
    value: f32,
}

fn ev(at: f64, key: &'static str, value: f32) -> Event {
    Event { at, key, value }
}

/// Max second difference of `x` over `range`: the size of the largest kink.
fn max_kink(x: &[f32], range: std::ops::Range<usize>) -> f64 {
    range
        .map(|i| (x[i + 1] - 2.0 * x[i] + x[i - 1]).abs() as f64)
        .fold(0.0, f64::max)
}

/// A 150 Hz tone with a smooth 250 ms fade-in, through a wet-only reverb
/// built from `cfg`, with `events` applied at block boundaries (1 ms
/// blocks). Returns the largest kink during the changes divided by the
/// larger kink of the settled signal before and after them.
fn click_ratio(cfg: &[(&str, f64)], events: &[Event], post: f64) -> f64 {
    let mut m = wet_module(cfg);
    let (t0, t_last) = (events[0].at, events.last().unwrap().at);
    let end = secs(t_last + post);
    let mut next = 0;
    let (l, r) = render(
        m.as_mut(),
        end,
        48,
        |i| {
            let fade = (i as f64 / (0.25 * SRF)).min(1.0);
            let env = 0.5 - 0.5 * (std::f64::consts::PI * fade).cos();
            let s = 0.3 * env * (2.0 * std::f64::consts::PI * 150.0 * i as f64 / SRF).sin();
            (s as f32, 0.0)
        },
        false,
        |i, m| {
            while next < events.len() && secs(events[next].at) <= i {
                m.set_control(events[next].key, events[next].value).unwrap();
                next += 1;
            }
        },
    );
    let mut worst = 0.0f64;
    for x in [&l, &r] {
        assert!(all_finite(x));
        let settled =
            max_kink(x, secs(t0 - 1.0)..secs(t0)).max(max_kink(x, end - secs(1.0)..end - 1));
        assert!(settled > 0.0, "tone is silent");
        let changing = max_kink(x, secs(t0)..secs(t_last + 0.5));
        worst = worst.max(changing / settled);
    }
    worst
}

/// Largest change-window kink allowed, in units of the settled signal's own.
const KINK_LIMIT: f64 = 10.0;

fn assert_click_free(name: &str, ratio: f64) {
    println!("{name}: kink ratio {ratio:.2}");
    assert!(
        ratio < KINK_LIMIT,
        "{name}: kink {ratio:.1}x the settled signal's"
    );
}

/// Criterion 3: step changes of room_size are click-free.
#[test]
fn room_size_steps_are_click_free() {
    let base = [("decay", 0.2), ("room_size", 0.5)];
    for (name, to) in [("up", 0.9), ("down", 0.1)] {
        let ratio = click_ratio(&base, &[ev(3.0, "room_size", to)], 4.0);
        assert_click_free(&format!("room_size step {name}"), ratio);
    }
}

/// Criterion 3: step changes of decay are click-free.
#[test]
fn decay_steps_are_click_free() {
    for (name, from, to) in [("up", 0.2, 0.8), ("down", 0.8, 0.1)] {
        let ratio = click_ratio(&[("decay", from)], &[ev(4.0, "decay", to)], 4.0);
        assert_click_free(&format!("decay step {name}"), ratio);
    }
}

/// Criterion 3: slow ramps (a write every 10 ms, each landing inside the
/// previous crossfade) are click-free.
#[test]
fn room_size_and_decay_ramps_are_click_free() {
    let ramp = |key: &'static str, from: f32, to: f32| -> Vec<Event> {
        (0..=200)
            .map(|k| {
                ev(
                    3.0 + 0.01 * k as f64,
                    key,
                    from + (to - from) * k as f32 / 200.0,
                )
            })
            .collect()
    };
    let ratio = click_ratio(
        &[("decay", 0.2), ("room_size", 0.3)],
        &ramp("room_size", 0.3, 0.7),
        4.0,
    );
    assert_click_free("room_size ramp", ratio);
    let ratio = click_ratio(
        &[("decay", 0.2), ("room_size", 0.5)],
        &ramp("decay", 0.2, 0.8),
        4.0,
    );
    assert_click_free("decay ramp", ratio);
}

/// Criterion 3: changes arriving in the middle of a crossfade (10-15 ms
/// apart, inside the ~30 ms fade) are click-free, including room and decay
/// together.
#[test]
fn changes_during_a_crossfade_are_click_free() {
    let burst = |key: &'static str, values: [f32; 4]| -> Vec<Event> {
        [3.0, 3.010, 3.025, 3.040]
            .iter()
            .zip(values)
            .map(|(&t, v)| ev(t, key, v))
            .collect()
    };
    let ratio = click_ratio(
        &[("decay", 0.3)],
        &burst("room_size", [0.2, 0.8, 0.35, 0.6]),
        4.0,
    );
    assert_click_free("room_size mid-crossfade", ratio);
    let ratio = click_ratio(
        &[("decay", 0.5)],
        &burst("decay", [0.1, 0.9, 0.3, 0.5]),
        4.0,
    );
    assert_click_free("decay mid-crossfade", ratio);
    let mixed = [
        ev(3.0, "room_size", 0.2),
        ev(3.008, "decay", 0.9),
        ev(3.016, "room_size", 0.8),
        ev(3.024, "decay", 0.1),
        ev(3.032, "room_size", 0.5),
    ];
    assert_click_free(
        "room+decay mid-crossfade",
        click_ratio(&[("decay", 0.3)], &mixed, 4.0),
    );
}

/// Criterion 3: freezing and unfreezing is click-free.
#[test]
fn freeze_toggle_is_click_free() {
    let toggles = [ev(3.0, "freeze", 1.0), ev(4.5, "freeze", 0.0)];
    let ratio = click_ratio(&[("decay", 0.2)], &toggles, 4.0);
    assert_click_free("freeze toggle", ratio);
}

/// Sensitivity check for the kink metric: a 1% step in a 150 Hz tone sits
/// well above the limit, so the click tests above could not miss one.
#[test]
fn kink_metric_catches_a_small_step() {
    let tone = |i: usize| 0.3 * (2.0 * std::f64::consts::PI * 150.0 * i as f64 / SRF).sin();
    let clean: Vec<f32> = (0..SR as usize).map(|i| tone(i) as f32).collect();
    let mut stepped = clean.clone();
    for v in &mut stepped[SR as usize / 2..] {
        *v += 0.003;
    }
    let settled = max_kink(&clean, 1..SR as usize - 1);
    let ratio = max_kink(&stepped, 1..SR as usize - 1) / settled;
    assert!(ratio > 2.0 * KINK_LIMIT, "ratio {ratio}");
}
