//! Criterion 2: no blow-up at maximum decay or with freeze.

use super::rig::*;
use super::*;

/// Criterion 2: no blow-up at maximum decay with sustained noise.
#[test]
fn max_decay_sustained_noise_stays_bounded() {
    for room in [0.0, 0.5, 1.0] {
        let mut m = wet_module(&[("decay", 1.0), ("damping", 0.0), ("room_size", room)]);
        let mut noise = Noise(0x1234_5678);
        let (l, r) = render(
            m.as_mut(),
            secs(24.0),
            BLOCK,
            |_| (0.5 * noise.next(), 0.0),
            false,
            no_hook,
        );
        assert!(all_finite(&l) && all_finite(&r), "room {room}: non-finite");
        let p = peak(&l).max(peak(&r));
        let (a, b) = (rms(&l[secs(12.0)..secs(18.0)]), rms(&l[secs(18.0)..]));
        println!("room {room}: peak {p:.3}, rms {a:.4} -> {b:.4}");
        assert!(p < MAX_PEAK, "room {room}: peak {p}");
        assert!(a > 1e-4, "room {room}: silent");
        assert!(
            (0.5..2.0).contains(&(b / a)),
            "room {room}: level drifts {a} -> {b} under steady input"
        );
    }
}

/// Renders a frozen tail: a noise burst, freeze at 1 s, `total` seconds.
/// `extra` is an optional burst injected at 5 s while frozen.
fn frozen_render(damping: f64, total: f64, extra: bool) -> (Vec<f32>, Vec<f32>) {
    let mut m = wet_module(&[("damping", damping)]);
    let mut noise = Noise(0xBEEF_1234);
    render(
        m.as_mut(),
        secs(total),
        BLOCK,
        |i| {
            let burst = i < secs(0.3) || (extra && (secs(5.0)..secs(5.5)).contains(&i));
            (if burst { 0.5 * noise.next() } else { 0.0 }, 0.0)
        },
        false,
        |i, m| {
            if i >= secs(1.0) && i < secs(1.0) + BLOCK {
                m.set_control("freeze", 1.0).unwrap();
            }
        },
    )
}

/// Criterion 2: with freeze on, the tail neither dies nor grows over 30 s,
/// even with damping at maximum (freeze bypasses damping).
#[test]
fn freeze_sustains_tail_without_growth() {
    for damping in [0.0, 1.0] {
        let (l, r) = frozen_render(damping, 33.0, false);
        assert!(all_finite(&l) && all_finite(&r));
        assert!(peak(&l).max(peak(&r)) < MAX_PEAK);
        let early = rms(&l[secs(2.0)..secs(4.0)]);
        let late = rms(&l[secs(31.0)..secs(33.0)]);
        println!("damping {damping}: frozen rms {early:.4} -> {late:.4}");
        assert!(early > 1e-4, "frozen tail is silent");
        assert!(
            (0.5..2.0).contains(&(late / early)),
            "damping {damping}: frozen level {early} -> {late}"
        );
    }
}

/// Criterion 2: freeze mutes the input to the network: a burst fed while
/// frozen changes nothing.
#[test]
fn freeze_ignores_new_input() {
    let (a, _) = frozen_render(0.5, 8.0, false);
    let (b, _) = frozen_render(0.5, 8.0, true);
    let window = secs(1.5)..secs(8.0);
    let worst = a[window.clone()]
        .iter()
        .zip(&b[window])
        .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
    assert!(
        worst <= 1e-4 * peak(&a),
        "input leaked into frozen tail: {worst}"
    );
}
