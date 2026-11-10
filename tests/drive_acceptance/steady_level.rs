//! Criterion: "With auto gain on, a -12 dBFS sine's output RMS stays within
//! +/-3 dB across the full drive range." Applies to the curves and `bits`;
//! `rate` holds samples and is exempt. Auto gain is the `auto_level` control.

use super::rig::*;
use super::*;

fn level_error_db(rig: &mut Rig, freq: f64, settle: f64) -> f64 {
    let s = sine(10f64.powf(-12.0 / 20.0), freq);
    rig.run(secs(settle), &s);
    let t0 = rig.t;
    let out = rig.run(secs(0.25), &s);
    let input: Vec<f32> = (0..out.len()).map(|i| s(t0 + i)).collect();
    20.0 * (rms(&out) / rms(&input)).log10()
}

/// With `auto_level` on, sweeping `drive` live on one instance (set_control, 0.6 s to
/// settle after each step) keeps a -12 dBFS sine's RMS within 3 dB.
#[test]
fn auto_level_holds_rms_within_3_db_when_drive_is_swept_live() {
    for shape in LEVEL_KEEPING {
        let mut rig = Rig::new(json!({ "shape": shape, "drive": 0.0,
            "auto_level": true, "mix": 1.0, "level": 1.0 }));
        for step in 0..=20 {
            let drive = step as f32 / 20.0;
            rig.set("drive", drive);
            let err = level_error_db(&mut rig, 480.0, 0.6);
            assert!(err.abs() <= 3.0, "{shape} live drive {drive}: {err:+.2} dB");
        }
    }
}
