//! The registry surface: ports, declared controls, and config.

use super::rig::*;
use super::*;

/// Registered as `limiter`, with `audio_left`/`audio_right` in and out.
#[test]
fn limiter_has_stereo_ports() {
    assert!(ModuleRegistry::default().has_type("limiter"));
    let lim = build("limiter", json!({}));
    for port in ["audio_left", "audio_right"] {
        assert!(lim.m().input_port_index(port).is_some(), "input {port}");
    }
    assert_eq!(lim.m().outputs(), ["audio_left", "audio_right"]);
}

/// Criterion: "gain reduction is readable as a control (`reduction`)": the
/// controls are declared with the pinned ranges and defaults.
#[test]
fn controls_are_declared_with_their_ranges_and_defaults() {
    let lim = build("limiter", json!({}));
    assert_number(&lim, "ceiling", -24.0, 0.0, -1.0);
    assert_number(&lim, "release", 0.001, 2.0, 0.1);
    assert_number(&lim, "reduction", 0.0, 96.0, 0.0);
    assert_eq!(lim.get("ceiling"), -1.0);
    assert!((lim.get("release") - 0.1).abs() < 1e-6);
}

/// Controls start from config, are writable live, and clamp into range;
/// an unknown config key is refused.
#[test]
fn controls_start_from_config_clamp_and_config_is_closed() {
    let mut lim = build(
        "limiter",
        json!({ "ceiling": -6.0, "release": 0.5, "lookahead": 0.01 }),
    );
    assert_eq!(lim.get("ceiling"), -6.0);
    assert!((lim.get("release") - 0.5).abs() < 1e-6);
    lim.set("ceiling", 5.0);
    assert_eq!(lim.get("ceiling"), 0.0, "ceiling clamps high");
    lim.set("ceiling", -40.0);
    assert_eq!(lim.get("ceiling"), -24.0, "ceiling clamps low");
    lim.set("release", 10.0);
    assert_eq!(lim.get("release"), 2.0, "release clamps high");
    assert!(try_build("limiter", json!({ "no_such_key": 1.0 })).is_err());
}
