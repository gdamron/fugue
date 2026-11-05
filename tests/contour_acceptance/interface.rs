//! The pinned interface: ports, controls, choices by name, defaults.

use super::rig::*;
use super::*;

fn number(c: &Contour, key: &str) -> (f32, f32, f32) {
    let meta = c
        .controls
        .iter()
        .find(|m| m.key == key)
        .unwrap_or_else(|| panic!("control {key} is declared"));
    let ControlValue::Number(default) = meta.default else {
        panic!("{key} default is a number");
    };
    let ControlKind::Number { min, max, .. } = meta.kind else {
        panic!("{key} is a number control");
    };
    (min, max, default)
}

/// Builds with no config; has the optional inputs and the four outputs.
#[test]
fn builds_with_no_config_and_has_the_specified_ports() {
    let c = contour(json!({}));
    let m = c.module.module();
    for name in [
        "trigger",
        "clock",
        "frequency",
        "frequency_mod",
        "shape_mod",
        "slope_mod",
        "smoothness_mod",
    ] {
        assert!(
            m.inputs().contains(&name),
            "input {name} in {:?}",
            m.inputs()
        );
    }
    for name in ["contour.0", "contour.1", "contour.2", "contour.3"] {
        assert!(
            m.outputs().contains(&name),
            "output {name} in {:?}",
            m.outputs()
        );
    }
}

/// Numeric controls declare the specified ranges and defaults.
#[test]
fn numeric_controls_have_specified_ranges_and_defaults() {
    let c = contour(json!({}));
    assert_eq!(number(&c, "frequency"), (0.001, 20000.0, 1.0));
    for key in ["shape", "slope", "smoothness", "shift"] {
        assert_eq!(number(&c, key), (0.0, 1.0, 0.5), "{key}");
    }
    assert_eq!(number(&c, "frequency_mod_depth").2, 1.0);
    for key in ["shape_mod_depth", "slope_mod_depth", "smoothness_mod_depth"] {
        assert_eq!(number(&c, key), (0.0, 1.0, 1.0), "{key}");
    }
}

/// Choices are named, default to looping / control / phase, and take their
/// starting value by name from config.
#[test]
fn choices_have_named_options_defaults_and_config() {
    let choices = [
        ("mode", vec!["ad", "looping", "ar"], "looping", "ar"),
        ("range", vec!["control", "audio"], "control", "audio"),
        (
            "output_mode",
            vec!["envelope", "amplitude", "phase", "ratio"],
            "phase",
            "ratio",
        ),
    ];
    let c = contour(json!({}));
    for (key, names, default, _) in &choices {
        for name in names {
            option_position(&c.controls, key, name);
        }
        let meta = c.controls.iter().find(|m| m.key == *key).unwrap();
        let ControlKind::String { options: Some(o) } = &meta.kind else {
            panic!("{key} is a choice");
        };
        assert_eq!(o.len(), names.len(), "{key} options {o:?}");
        let want = option_position(&c.controls, key, default);
        assert_eq!(
            c.module.module().get_control(key),
            Ok(want),
            "{key} default"
        );
    }
    let c = contour(
        json!({ "mode": "ar", "range": "audio", "output_mode": "ratio",
                            "frequency": 7.0, "shape": 0.25 }),
    );
    for (key, _, _, set) in &choices {
        let want = option_position(&c.controls, key, set);
        assert_eq!(c.module.module().get_control(key), Ok(want), "{key} config");
    }
    assert_eq!(c.module.module().get_control("frequency"), Ok(7.0));
    assert_eq!(c.module.module().get_control("shape"), Ok(0.25));
}
