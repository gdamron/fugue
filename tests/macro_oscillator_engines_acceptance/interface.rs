//! The engines are appended to the `model` control, build by name, and leave
//! the module's interface unchanged.

use super::rig::*;
use super::*;

/// Criterion: "appended to the `model` control's options, after the seven
/// existing models, in this order: string, modal, bass_drum, snare_drum,
/// hi_hat, filtered_noise, particle_noise".
#[test]
fn model_options_append_the_new_engines_in_order() {
    let models = voice(json!({})).models;
    let expected: Vec<&str> = EXISTING.iter().chain(NEW.iter()).copied().collect();
    assert_eq!(models, expected, "model options");
}

/// Criterion: "builds with `{"model": "<name>"}` in config", and the model
/// control then reads back as that option's position.
mod builds_by_name {
    use super::*;

    fn check(name: &str) {
        let v = engine(name);
        let position = EXISTING.len() + NEW.iter().position(|n| *n == name).unwrap();
        assert_eq!(v.module.module().get_control("model"), Ok(position as f32));
    }

    each_new_model!(check);
}

/// Criterion: "the module's interface is unchanged": the same inputs, outputs
/// and control keys, and `morph` still spans 0..1 with default 0.5.
#[test]
fn interface_is_unchanged() {
    let v = voice(json!({}));
    let m = v.module.module();
    for name in ["frequency", "frequency_mod", "trigger", "level"] {
        assert!(
            m.inputs().contains(&name),
            "input {name} in {:?}",
            m.inputs()
        );
    }
    for name in ["audio", "aux"] {
        assert!(
            m.outputs().contains(&name),
            "output {name} in {:?}",
            m.outputs()
        );
    }
    for key in [
        "model",
        "frequency",
        "frequency_mod_depth",
        "harmonics",
        "timbre",
        "morph",
        "level",
        "decay",
        "key",
        "scale",
    ] {
        assert!(v.control(key).is_some(), "control {key} is declared");
    }
    let morph = v.control("morph").unwrap();
    assert_eq!(morph.default, ControlValue::Number(0.5));
    assert!(matches!(
        morph.kind,
        ControlKind::Number { min, max, .. } if min == 0.0 && max == 1.0
    ));
}

/// Criterion: every new engine makes sound when un-triggered (the module's
/// own continuous-excitation default) and when triggered, on `audio`.
mod makes_sound {
    use super::*;

    fn check(name: &str) {
        let mut v = engine(name);
        let x = v.render(seconds(1.0));
        assert!(
            peak(&x[seconds(0.1)..]) > 1e-3,
            "{name} is silent un-triggered"
        );
        let mut v = engine(name);
        let (x, _) = v.render_driven(seconds(1.0), pulses(&[T0]));
        assert!(peak(&x) > 0.01, "{name} is silent after a trigger");
    }

    each_new_model!(check);
}
