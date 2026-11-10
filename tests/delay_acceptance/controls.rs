//! Live-path control writes: config starting values and live reads and
//! writes through a running invention.

use super::rig::*;
use super::*;

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
