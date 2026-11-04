//! Interface: ports, default controls and the gate edge timing.

use crate::rig::*;
use fugue::ModuleRegistry;

#[test]
fn interface_ports_and_default_controls_match_the_contract() {
    let built = ModuleRegistry::default()
        .build("chance", SAMPLE_RATE, &serde_json::json!({}))
        .expect("chance builds with an empty config");
    let m = built.module.module();
    for name in ["clock", "reset"] {
        assert!(m.input_port_index(name).is_some(), "input `{name}`");
    }
    for name in OUTPUTS {
        assert!(m.output_port_index(name).is_some(), "output `{name}`");
    }

    let surface = built.control_surface.expect("a control surface");
    let strings = [
        ("mode", "complementary"),
        ("rate", "x1"),
        ("key", "C"),
        ("scale", "major"),
    ];
    for (key, expected) in strings {
        let got = surface
            .get_control(key)
            .unwrap_or_else(|e| panic!("{key}: {e}"));
        assert_eq!(got.as_string().unwrap(), expected, "default of `{key}`");
    }
    let numbers = [
        ("bias", 0.5),
        ("jitter", 0.0),
        ("spread", 0.5),
        ("value_bias", 0.5),
        ("steps", 0.5),
        ("low_note", 48.0),
        ("range", 24.0),
        ("repetition", 0.0),
        ("loop_length", 8.0),
        ("seed", -1.0),
    ];
    for (key, expected) in numbers {
        let got = surface
            .get_control(key)
            .unwrap_or_else(|e| panic!("{key}: {e}"));
        assert_eq!(got.as_number().unwrap(), expected, "default of `{key}`");
    }
}

#[test]
fn a_fired_gate_rises_on_the_clock_edge_sample_and_holds_while_the_clock_is_high() {
    // Interface: "At rate x1 with jitter 0, a fired gate rises on the same
    // sample as the clock edge and stays high while the clock input stays high."
    let mut r = rig(serde_json::json!({ "bias": 0.5, "jitter": 0.0, "seed": 31 }));
    let mut fired = 0;
    for _ in 0..200 {
        r.block([1.0, 1.0, 1.0, 0.0], [0.0; BLOCK]);
        let on_edge = r.snapshot(0);
        for held_at in 1..3 {
            let held = r.snapshot(held_at);
            assert_eq!(on_edge.gate[0].to_bits(), held.gate[0].to_bits());
            assert_eq!(on_edge.gate[1].to_bits(), held.gate[1].to_bits());
        }
        assert!(on_edge.tick > 0.5, "tick is a gate on every step");
        for g in on_edge.gate {
            assert!(g == 0.0 || g == 1.0, "gates are 0.0 or 1.0, got {g}");
        }
        fired += on_edge.fired(0) as usize + on_edge.fired(1) as usize;
        // Let the clock fall fully before the next edge.
        r.block([0.0; BLOCK], [0.0; BLOCK]);
    }
    assert_eq!(
        fired, 200,
        "complementary mode fires exactly one gate per step"
    );
}
