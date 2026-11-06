//! The declared ports and control surface, as pinned in the interface.

use super::rig::*;
use super::*;

pub const DIVISIONS: [&str; 15] = [
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

#[test]
fn ports_are_stereo_audio_plus_a_bpm_input() {
    let unit = build(json!({}));
    for port in ["audio_left", "audio_right", "bpm"] {
        assert!(unit.m().input_port_index(port).is_some(), "input {port}");
    }
    assert_eq!(unit.m().outputs(), ["audio_left", "audio_right"]);
}

/// Keys, ranges and defaults as listed in the interface table; no other
/// controls exist.
#[test]
fn controls_have_the_stated_ranges_and_defaults() {
    let unit = build(json!({}));
    let numbers = [
        ("stage_count", 4.0, 12.0, 6.0),
        ("rate", 0.01, 10.0, 0.5),
        ("bpm", 1.0, 1000.0, 120.0),
        ("depth", 0.0, 1.0, 0.6),
        ("frequency", 50.0, 4000.0, 700.0),
        ("feedback", -0.95, 0.95, 0.3),
        ("stereo_phase", 0.0, 1.0, 0.25),
        ("mix", 0.0, 1.0, 0.5),
    ];
    for (key, lo, hi, default) in numbers {
        let meta = unit.meta(key);
        let ControlKind::Number { min, max, .. } = meta.kind else {
            panic!("{key} is a number control, got {:?}", meta.kind)
        };
        assert_eq!((min, max), (lo, hi), "{key} range");
        let ControlValue::Number(d) = meta.default else {
            panic!("{key} default is a number")
        };
        assert!((d - default).abs() < 1e-6, "{key} default {d}");
    }
    let division = unit.meta("division");
    let ControlKind::String { options: Some(got) } = &division.kind else {
        panic!("division is a choice")
    };
    assert_eq!(got, &DIVISIONS, "division options");
    assert_eq!(division.default, ControlValue::from("off"));
    let mut keys: Vec<String> = unit.m().controls().into_iter().map(|c| c.key).collect();
    keys.sort();
    let mut want: Vec<&str> = numbers.iter().map(|n| n.0).collect();
    want.push("division");
    want.sort();
    assert_eq!(keys, want, "the control set");
}

/// Every control may be given in config as its starting value and reads back
/// as written.
#[test]
fn config_keys_are_the_controls_and_start_the_module() {
    let config = json!({ "stage_count": 9, "rate": 2.5, "division": "8th_note", "bpm": 90.0,
        "depth": 0.4, "frequency": 1200.0, "feedback": -0.5, "stereo_phase": 0.5, "mix": 0.8 });
    let built = ModuleRegistry::default()
        .build("phaser", SR, &config)
        .expect("phaser builds from every control as a config key");
    let m = built.module.module();
    for (key, want) in [
        ("stage_count", 9.0f32),
        ("rate", 2.5),
        ("bpm", 90.0),
        ("depth", 0.4),
        ("frequency", 1200.0),
        ("feedback", -0.5),
        ("stereo_phase", 0.5),
        ("mix", 0.8),
    ] {
        let got = m.get_control(key).unwrap_or_else(|e| panic!("{key}: {e}"));
        assert!((got - want).abs() < 1e-6, "{key} = {got}");
    }
}

/// Assumption: "no other keys" means an unknown config key is refused, the
/// way every other module refuses one.
#[test]
fn an_unknown_config_key_is_refused() {
    try_build(json!({})).expect("phaser builds with no config");
    assert!(try_build(json!({ "no_such_key": 1.0 })).is_err());
}
