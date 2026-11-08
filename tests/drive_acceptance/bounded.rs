//! Criterion: "Output stays within +/-1." Every shape is driven hard and the
//! output must be finite with |y| <= 1.

use super::rig::*;
use super::*;

/// Hot inputs (above +/-1), noise and a +/-100 square, with extreme drive,
/// bias, tone and oversampling settings, auto level on and off, mix 1.
#[test]
fn hot_input_and_extreme_settings_stay_within_unity() {
    let settings = [
        (1.0, 1.0, 1.0, 1.0),
        (1.0, -1.0, -1.0, -1.0),
        (0.5, 0.7, 1.0, -1.0),
        (0.5, -0.7, -1.0, 1.0),
        (0.0, 0.0, 0.0, 0.0),
    ];
    for shape in SHAPES {
        for os in [1, 4] {
            for auto in [true, false] {
                for (drive, bias, pre, post) in settings {
                    for kind in 0..3 {
                        let mut rig = Rig::new(json!({ "shape": shape, "oversampling": os,
                            "drive": drive, "bias": bias, "pre_tone": pre, "post_tone": post,
                            "auto_level": auto, "level": 1.0, "mix": 1.0 }));
                        let mut n = noise(7);
                        let hot = sine(8.0, 777.0);
                        let input: Vec<f32> = (0..secs(0.2))
                            .map(|i| match kind {
                                0 => hot(i),
                                1 => 4.0 * n(),
                                _ => {
                                    if (i / 40) % 2 == 0 {
                                        100.0
                                    } else {
                                        -100.0
                                    }
                                }
                            })
                            .collect();
                        let out = rig.run(input.len(), &|i| input[i]);
                        let bad = out.iter().find(|y| !y.is_finite() || y.abs() > 1.0);
                        assert!(
                            bad.is_none(),
                            "{shape} os {os} auto {auto} drive {drive} bias {bias} pre {pre} \
                             post {post} input kind {kind}: {bad:?}"
                        );
                    }
                }
            }
        }
    }
}

/// With in-range input (full-scale sine and square) and the dry path mixed
/// in (mix 0, 0.5, 1), the blend stays within +/-1 too. Hot input is not
/// used here: a dry path may legitimately pass it through.
#[test]
fn partial_mix_of_full_scale_input_stays_within_unity() {
    for shape in SHAPES {
        for mix in [0.0, 0.5, 1.0] {
            for square in [false, true] {
                let mut rig = Rig::new(json!({ "shape": shape, "drive": 1.0, "bias": 0.5,
                    "mix": mix, "level": 1.0, "post_tone": 1.0 }));
                let sig = move |i: usize| {
                    if square {
                        if (i / 50).is_multiple_of(2) {
                            1.0
                        } else {
                            -1.0
                        }
                    } else {
                        sine(1.0, 440.0)(i)
                    }
                };
                let out = rig.run(secs(0.2), &sig);
                let bad = out.iter().find(|y| !y.is_finite() || y.abs() > 1.0);
                assert!(bad.is_none(), "{shape} mix {mix} square {square}: {bad:?}");
            }
        }
    }
}
