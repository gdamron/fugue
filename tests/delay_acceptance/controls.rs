//! The declared control surface and ports (the definition of done's
//! "controls declared" and live-path items).

use super::rig::*;
use super::*;

const DIVISIONS: [&str; 15] = [
    "off",
    "whole_note",
    "half_note",
    "quarter_note",
    "8th_note",
    "16th_note",
    "32nd_note",
    "dotted_half_note",
    "dotted_quarter_note",
    "dotted_8th_note",
    "dotted_16th_note",
    "half_triplet",
    "quarter_triplet",
    "8th_triplet",
    "16th_triplet",
];

/// Keys, kinds, ranges and defaults as listed in the interface table, and
/// the atom's ports.
#[test]
fn delay_declares_its_controls_and_ports() {
    let built = ModuleRegistry::default()
        .build("delay", SR, &json!({}))
        .unwrap();
    for port in ["audio", "time_mod", "bpm"] {
        assert!(
            built.module.module().input_port_index(port).is_some(),
            "input {port}"
        );
    }
    assert_eq!(built.module.module().outputs(), ["audio"]);
    let controls = built
        .control_surface
        .expect("delay has a control surface")
        .controls();
    let find = |key: &str| {
        controls
            .iter()
            .find(|m| m.key == key)
            .unwrap_or_else(|| panic!("control {key}"))
    };
    for (key, min, max, default) in [
        ("time", 0.0, 60.0, 0.25),
        ("bpm", 1.0, 1000.0, 120.0),
        ("feedback", 0.0, 1.1, 0.0),
        ("time_mod_depth", 0.0, 1.0, 0.005),
    ] {
        let meta = find(key);
        let ControlKind::Number {
            min: lo, max: hi, ..
        } = meta.kind
        else {
            panic!("{key} is a number")
        };
        assert_eq!((lo, hi), (min, max), "{key} range");
        let ControlValue::Number(got) = meta.default else {
            panic!("{key} default is a number")
        };
        assert!((got - default).abs() < 1e-6, "{key} default {got}");
    }
    assert_eq!(
        (&find("hold").kind, &find("hold").default),
        (&ControlKind::Bool, &ControlValue::Bool(false))
    );
    for (key, options, default) in [
        ("division", &DIVISIONS[..], "off"),
        ("glide", &["crossfade", "slew"][..], "crossfade"),
    ] {
        let ControlKind::String { options: Some(got) } = &find(key).kind else {
            panic!("{key} is a choice")
        };
        assert_eq!(got, options, "{key} options");
        assert_eq!(
            find(key).default,
            ControlValue::from(default),
            "{key} default"
        );
    }
}

/// Every control may be given in config as its starting value, and is
/// readable and writable in a running invention.
#[test]
fn controls_start_from_config_and_can_be_written_live() {
    let config = json!({ "time": 0.5, "division": "dotted_8th_note", "bpm": 90.0, "feedback": 0.25,
        "glide": "slew", "hold": false, "time_mod_depth": 0.02 });
    let engine = engine(tap_doc(noise_source(0.5), "white", config));
    let get = |key: &str| {
        engine
            .get_control("d", key)
            .unwrap_or_else(|e| panic!("{key}: {e}"))
    };
    for (key, want) in [
        ("time", 0.5f32),
        ("bpm", 90.0),
        ("feedback", 0.25),
        ("time_mod_depth", 0.02),
    ] {
        let ControlValue::Number(got) = get(key) else {
            panic!("{key} is a number")
        };
        assert!((got - want).abs() < 1e-6, "{key} = {got}");
    }
    assert_eq!(get("division"), ControlValue::from("dotted_8th_note"));
    assert_eq!(get("glide"), ControlValue::from("slew"));
    set(&engine, "d", "division", "8th_triplet");
    set(&engine, "d", "glide", "crossfade");
    set(&engine, "d", "hold", true);
    assert_eq!(get("division"), ControlValue::from("8th_triplet"));
    assert_eq!(get("glide"), ControlValue::from("crossfade"));
    assert_eq!(get("hold"), ControlValue::Bool(true));
    assert!(engine
        .set_control("d", "division", ControlValue::from("no_such_note"))
        .is_err());
}
