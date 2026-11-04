//! Definition of done: "controls are declared", plus the module's registry
//! surface (ports and config).

use super::rig::*;
use super::*;

/// The module is registered as `drive` with one `audio` input and one `audio`
/// output, and builds for each shape name and oversampling of 1, 2 and 4.
#[test]
fn registry_has_drive_with_audio_ports_and_builds_for_every_config() {
    let registry = ModuleRegistry::default();
    assert!(registry.has_type("drive"));
    let rig = Rig::new(json!({}));
    assert_eq!(rig.m.inputs(), ["audio"]);
    assert_eq!(rig.m.outputs(), ["audio"]);
    for shape in SHAPES {
        for os in [1, 2, 4] {
            Rig::new(json!({ "shape": shape, "oversampling": os }));
        }
    }
}

fn meta(rig: &Rig, key: &str) -> fugue::ControlMeta {
    rig.m
        .controls()
        .into_iter()
        .find(|c| c.key == key)
        .unwrap_or_else(|| panic!("drive declares a {key} control"))
}

/// Every named control is declared with its documented range or type and
/// default, and numeric ones read back what was set.
#[test]
fn controls_are_declared_with_documented_ranges_and_defaults() {
    let mut rig = Rig::new(json!({}));
    let numbers = [
        ("drive", 0.0, 1.0, Some(0.25)),
        ("pre_tone", -1.0, 1.0, None),
        ("post_tone", -1.0, 1.0, None),
        ("bias", -1.0, 1.0, None),
        ("level", 0.0, 1.0, Some(1.0)),
        ("mix", 0.0, 1.0, Some(1.0)),
    ];
    for (key, lo, hi, default) in numbers {
        let m = meta(&rig, key);
        match m.kind {
            ControlKind::Number { min, max, .. } => assert_eq!((min, max), (lo, hi), "{key}"),
            other => panic!("{key} is a number control, got {other:?}"),
        }
        if let Some(d) = default {
            assert_eq!(m.default, ControlValue::Number(d), "{key} default");
        }
        let v = (lo + hi) / 2.0 + 0.1;
        rig.set(key, v);
        assert!(
            (rig.m.get_control(key).unwrap() - v).abs() < 1e-6,
            "{key} reads back"
        );
    }
    assert_eq!(meta(&rig, "auto_level").default, ControlValue::Bool(true));
    assert!(matches!(meta(&rig, "auto_level").kind, ControlKind::Bool));
}

/// `shape` is a choice of soft, hard, fold, asymmetric, bits and rate (in
/// that order), and accepts live numeric indices 0 to 5.
#[test]
fn shape_is_a_choice_of_the_six_shapes_settable_by_index() {
    let mut rig = Rig::new(json!({}));
    match meta(&rig, "shape").kind {
        ControlKind::String {
            options: Some(options),
        } => assert_eq!(options, SHAPES),
        other => panic!("shape is a choice control, got {other:?}"),
    }
    for i in 0..6 {
        rig.set("shape", i as f32);
    }
}
