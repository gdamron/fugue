//! Criterion 4: stereo decorrelation and the wet/dry/width mix.

use super::rig::*;
use super::*;

/// Criterion 4: with width 1, a mono impulse decorrelates into L and R.
#[test]
fn width_one_decorrelates_impulse_tail() {
    for room in [0.3, 0.5, 0.8] {
        let mut m = wet_module(&[("room_size", room)]);
        let (l, r) = render(
            m.as_mut(),
            secs(3.0),
            BLOCK,
            |i| (if i == 0 { 1.0 } else { 0.0 }, 0.0),
            false,
            no_hook,
        );
        let rho = correlation(&l[secs(0.1)..], &r[secs(0.1)..]);
        println!("impulse room {room}: rho {rho:.3}");
        assert!(rho.abs() < 0.3, "room {room}: |rho| = {}", rho.abs());
    }
}

/// Criterion 4: with width 1, a mono noise input decorrelates into L and R.
#[test]
fn width_one_decorrelates_noise_tail() {
    for room in [0.3, 0.5, 0.8] {
        let mut m = wet_module(&[("room_size", room)]);
        let mut noise = Noise(0xC0FF_EE11);
        let (l, r) = render(
            m.as_mut(),
            secs(6.0),
            BLOCK,
            |_| (0.3 * noise.next(), 0.0),
            false,
            no_hook,
        );
        let rho = correlation(&l[secs(1.0)..], &r[secs(1.0)..]);
        println!("noise room {room}: rho {rho:.3}");
        assert!(rho.abs() < 0.3, "room {room}: |rho| = {}", rho.abs());
    }
}

/// Criterion 4: with width 0 the two wet outputs are identical.
#[test]
fn width_zero_outputs_are_identical() {
    let mut m = wet_module(&[("width", 0.0)]);
    let mut noise = Noise(7);
    let (l, r) = render(
        m.as_mut(),
        secs(2.0),
        BLOCK,
        |_| (0.4 * noise.next(), 0.0),
        false,
        no_hook,
    );
    assert!(rms(&l) > 1e-4, "there is a wet signal to compare");
    for (i, (a, b)) in l.iter().zip(&r).enumerate() {
        assert!((a - b).abs() <= 1e-6, "frame {i}: {a} vs {b}");
    }
}

/// Stereo noise rendered at `config` (everything else default): (L, R).
fn stereo_noise_render(config: Value) -> (Vec<f32>, Vec<f32>) {
    let mut m = build(config);
    let mut noise = Noise(99);
    render(
        m.as_mut(),
        secs(1.5),
        BLOCK,
        |_| (0.4 * noise.next(), 0.4 * noise.next()),
        true,
        no_hook,
    )
}

/// Criterion 4 / spec: the wet width law, same side wet * (0.5 + 0.5 * width),
/// cross wet * 0.5 * (1 - width), checked against the width-1 render.
#[test]
fn width_law_mixes_wet_channels() {
    let wet = 0.8;
    let (wl, wr) = stereo_noise_render(json!({"wet": wet, "dry": 0.0, "width": 1.0}));
    for width in [0.0, 0.25, 0.5, 0.75] {
        let (l, r) = stereo_noise_render(json!({"wet": wet, "dry": 0.0, "width": width}));
        let (same, cross) = ((0.5 + 0.5 * width) as f32, (0.5 * (1.0 - width)) as f32);
        let tol = 1e-4 * peak(&wl);
        for i in 0..l.len() {
            assert!(
                (l[i] - (same * wl[i] + cross * wr[i])).abs() <= tol,
                "width {width} L frame {i}"
            );
            assert!(
                (r[i] - (same * wr[i] + cross * wl[i])).abs() <= tol,
                "width {width} R frame {i}"
            );
        }
    }
}

/// Spec: wet is a linear gain on the reverberated signal.
#[test]
fn wet_is_a_linear_gain() {
    let (full, _) = stereo_noise_render(json!({"wet": 1.0, "dry": 0.0}));
    let (half, _) = stereo_noise_render(json!({"wet": 0.5, "dry": 0.0}));
    let tol = 1e-4 * peak(&full);
    for i in 0..full.len() {
        assert!((half[i] - 0.5 * full[i]).abs() <= tol, "frame {i}");
    }
}

/// Spec: dry is a linear gain on each channel's own input; wet 0 leaves only that.
#[test]
fn dry_is_a_linear_gain_per_channel() {
    let mut m = build(json!({"wet": 0.0, "dry": 0.5, "width": 1.0}));
    let (mut a, mut b) = (Noise(3), Noise(4));
    let (mut ins_l, mut ins_r) = (vec![], vec![]);
    let (l, r) = render(
        m.as_mut(),
        secs(0.5),
        BLOCK,
        |_| {
            let pair = (0.6 * a.next(), 0.6 * b.next());
            ins_l.push(pair.0);
            ins_r.push(pair.1);
            pair
        },
        true,
        no_hook,
    );
    for i in 0..l.len() {
        assert!((l[i] - 0.5 * ins_l[i]).abs() <= 1e-6, "L frame {i}");
        assert!((r[i] - 0.5 * ins_r[i]).abs() <= 1e-6, "R frame {i}");
    }
}

/// Spec: with the right input unconnected, the right channel copies the left
/// (mono in, stereo out), for the dry path and for the reverberated path.
#[test]
fn unconnected_right_input_copies_left() {
    let mut m = build(json!({"wet": 0.0, "dry": 1.0}));
    let mut noise = Noise(5);
    let (l, r) = render(
        m.as_mut(),
        secs(0.2),
        BLOCK,
        |_| (noise.next(), 0.0),
        false,
        no_hook,
    );
    assert!(rms(&r) > 0.1, "right output carries the mono input");
    for i in 0..l.len() {
        assert!((l[i] - r[i]).abs() <= 1e-6, "dry frame {i}");
    }
    // The wet network sees the same signal on both sides: feeding left only
    // equals feeding the same signal to both inputs.
    let drive = |stereo: bool| {
        let mut m = wet_module(&[]);
        let mut noise = Noise(6);
        render(
            m.as_mut(),
            secs(1.0),
            BLOCK,
            |_| {
                let s = 0.4 * noise.next();
                (s, s)
            },
            stereo,
            no_hook,
        )
    };
    let (mono_l, mono_r) = drive(false);
    let (st_l, st_r) = drive(true);
    for i in 0..mono_l.len() {
        assert!((mono_l[i] - st_l[i]).abs() <= 1e-5, "wet L frame {i}");
        assert!((mono_r[i] - st_r[i]).abs() <= 1e-5, "wet R frame {i}");
    }
}

/// Spec: a connected right input is its own channel (not a copy of the left).
#[test]
fn connected_right_input_is_independent() {
    let mut m = build(json!({"wet": 0.0, "dry": 1.0}));
    let (l, r) = render(
        m.as_mut(),
        secs(0.1),
        BLOCK,
        |i| (if i % 2 == 0 { 0.5 } else { -0.5 }, 0.25),
        true,
        no_hook,
    );
    assert!((l[10] - 0.5).abs() <= 1e-6 && (r[10] - 0.25).abs() <= 1e-6);
    assert!((r[11] - 0.25).abs() <= 1e-6);
}

/// Spec: non-finite output is replaced by 0, never passed on.
#[test]
fn non_finite_output_is_replaced_by_zero() {
    let mut m = build(json!({}));
    let (l, r) = render(
        m.as_mut(),
        secs(1.0),
        BLOCK,
        |i| match i {
            10 => (f32::NAN, f32::NAN),
            300 => (f32::INFINITY, f32::NEG_INFINITY),
            _ => (0.0, 0.0),
        },
        true,
        no_hook,
    );
    assert!(all_finite(&l) && all_finite(&r), "output must stay finite");
}
