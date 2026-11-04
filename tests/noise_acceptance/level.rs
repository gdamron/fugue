//! The `level` control and the module's registry surface.

use super::rig::*;
use super::*;

/// Criterion: the registry has `noise`, it builds with and without `seed`,
/// and it has outputs `white` and `pink` (this also covers the module catalog
/// criterion: catalog tests cover every registered type).
#[test]
fn registry_has_noise_with_white_and_pink_outputs() {
    let registry = ModuleRegistry::default();
    assert!(registry.has_type("noise"));
    assert!(registry.types().any(|t| t == "noise"));
    for config in [json!({}), json!({ "seed": 7 })] {
        registry
            .build("noise", SR, &config)
            .unwrap_or_else(|err| panic!("noise builds with {config}: {err}"));
    }
    let n = noise(json!({ "seed": 7 }));
    let outputs = n.module().outputs().to_vec();
    assert!(
        outputs.contains(&"white") && outputs.contains(&"pink"),
        "{outputs:?}"
    );
}

/// Criterion: "Control `level`, a number from 0 to 1 (default 1)": the
/// module declares a numeric `level` control spanning 0..1 whose default is 1,
/// and reads back 1 on a fresh instance.
#[test]
fn level_control_is_a_number_from_zero_to_one_defaulting_to_one() {
    let n = noise(json!({ "seed": 7 }));
    let meta = n
        .module()
        .controls()
        .into_iter()
        .find(|c| c.key == "level")
        .expect("noise declares a level control");
    assert_eq!(meta.default, ControlValue::Number(1.0));
    match meta.kind {
        ControlKind::Number { min, max, .. } => {
            assert_eq!((min, max), (0.0, 1.0), "level range");
        }
        other => panic!("level is a number control, got {other:?}"),
    }
    assert_eq!(n.level(), 1.0);
}

/// Criterion: "Level 0 silences both outputs", set live: after the glide
/// settles every sample of both outputs is zero (within 1e-6).
#[test]
fn level_zero_silences_both_outputs() {
    let mut n = seeded(7);
    n.run(seconds(0.1));
    n.set_level(0.0);
    n.run(seconds(0.1));
    let (white, pink) = n.render(seconds(1.0));
    assert!(peak(&white) <= 1e-6, "white peak {}", peak(&white));
    assert!(peak(&pink) <= 1e-6, "pink peak {}", peak(&pink));
    assert_eq!(n.level(), 0.0);
}

/// Criterion: level 0 given as a config value (a config key that sets a
/// control has the control's name) is silent from the first block.
/// Assumption: the `level` config key is accepted, per the control-naming
/// convention for config.
#[test]
fn level_zero_in_config_is_silent_from_the_start() {
    let (white, pink) = noise(json!({ "seed": 7, "level": 0.0 })).render(seconds(0.5));
    assert!(peak(&white) <= 1e-6, "white peak {}", peak(&white));
    assert!(peak(&pink) <= 1e-6, "pink peak {}", peak(&pink));
}

/// Criterion: level scales output between silence and full. Half level
/// is audibly quieter than full and louder than silence, on both outputs, and
/// restoring level 1 restores the full statistics (white RMS 0.577).
/// Assumption: the taper is not specified, so only monotonic ordering is
/// checked, not that 0.5 gives half the amplitude.
#[test]
fn intermediate_level_scales_between_silence_and_full() {
    let mut n = seeded(7);
    n.run(seconds(0.1));
    let (full_w, full_p) = n.render(seconds(2.0));
    n.set_level(0.5);
    n.run(seconds(0.1));
    let (half_w, half_p) = n.render(seconds(2.0));
    assert!(
        rms(&half_w) > 0.01 && rms(&half_w) < 0.95 * rms(&full_w),
        "white {}",
        rms(&half_w)
    );
    assert!(
        rms(&half_p) > 0.0 && rms(&half_p) < 0.95 * rms(&full_p),
        "pink {}",
        rms(&half_p)
    );
    n.set_level(1.0);
    n.run(seconds(0.1));
    let (back_w, _) = n.render(seconds(2.0));
    assert!(
        (rms(&back_w) - 0.577).abs() <= 0.02,
        "restored rms {}",
        rms(&back_w)
    );
}

/// Criterion: a level write settles. Going from level 1 to level 0
/// mid-stream, the output is silent within 50 ms.
/// Assumption: the spec says writes glide over a few milliseconds, so 50 ms
/// is generous.
#[test]
fn level_to_zero_settles_within_fifty_milliseconds() {
    let mut n = seeded(7);
    n.run(seconds(0.1));
    n.set_level(0.0);
    n.run(seconds(0.05));
    let (white, pink) = n.render(seconds(0.1));
    assert!(peak(&white) <= 1e-6 && peak(&pink) <= 1e-6);
}
