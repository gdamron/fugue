//! Integration tests for pull-based signal flow architecture.
//!
//! These tests verify that the new pull-based signal processing system
//! correctly handles various graph topologies and edge cases.

mod support;

use fugue::invention::Invention;
use fugue::modules::ClockControls;
use fugue::InventionBuilder;
use support::NullAudioBackend;

/// Test a simple chain: Clock → ADSR
#[test]
fn test_simple_chain() {
    let json = r#"
    {
        "name": "Simple Chain Test",
        "modules": [
            {
                "id": "clock",
                "type": "clock",
                "config": {"bpm": 120}
            },
            {
                "id": "adsr",
                "type": "adsr",
                "config": {
                    "attack": 0.01,
                    "decay": 0.1,
                    "sustain": 0.7,
                    "release": 0.2
                }
            },
            {
                "id": "dac",
                "type": "dac"
            }
        ],
        "connections": [
            {"from": "clock", "from_port": "gate", "to": "adsr", "to_port": "gate"},
            {"from": "adsr", "from_port": "envelope", "to": "dac", "to_port": "audio"}
        ]
    }
    "#;

    let invention: Invention = serde_json::from_str(json).expect("Failed to parse invention");
    let builder = InventionBuilder::new(44100);
    let (runtime, handles) = builder.build(invention).expect("Failed to build invention");
    let running = runtime
        .start_with_backend(NullAudioBackend::new(44100))
        .expect("Failed to start invention");

    // Should build without errors - actual audio playback not tested here
    let tempo: ClockControls = handles.get("clock.controls").expect("No tempo handle");
    assert_eq!(tempo.get_bpm(), 120.0);
    running.stop();
}

/// Test multi-input: Oscillator + ADSR → VCA
/// This is the simple_tone invention structure
#[test]
fn test_multi_input_vca() {
    let json = r#"
    {
        "name": "Multi-Input Test",
        "modules": [
            {
                "id": "clock",
                "type": "clock",
                "config": {"bpm": 120}
            },
            {
                "id": "adsr",
                "type": "adsr",
                "config": {
                    "attack": 0.01,
                    "decay": 0.1,
                    "sustain": 0.7,
                    "release": 0.3
                }
            },
            {
                "id": "osc",
                "type": "oscillator",
                "config": {"frequency": 440.0}
            },
            {
                "id": "vca",
                "type": "vca"
            },
            {
                "id": "dac",
                "type": "dac"
            }
        ],
        "connections": [
            {"from": "clock", "from_port": "gate", "to": "adsr", "to_port": "gate"},
            {"from": "adsr", "from_port": "envelope", "to": "vca", "to_port": "cv"},
            {"from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio"},
            {"from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio"}
        ]
    }
    "#;

    let invention: Invention = serde_json::from_str(json).expect("Failed to parse invention");
    let builder = InventionBuilder::new(44100);
    let (runtime, handles) = builder.build(invention).expect("Failed to build invention");
    let running = runtime
        .start_with_backend(NullAudioBackend::new(44100))
        .expect("Failed to start invention");

    // Should build successfully - the pull-based system should handle this correctly
    let tempo: ClockControls = handles.get("clock.controls").expect("No tempo handle");
    assert_eq!(tempo.get_bpm(), 120.0);
    running.stop();
}

/// Test diamond pattern: Clock feeds both Melody and ADSR
/// This tests that shared sources are processed correctly
#[test]
fn test_diamond_pattern() {
    let json = r#"
    {
        "name": "Diamond Pattern Test",
        "modules": [
            {
                "id": "clock",
                "type": "clock",
                "config": {"bpm": 120}
            },
            {
                "id": "melody",
                "type": "melody",
                "config": {
                    "root_note": 60,
                    "mode": "dorian",
                    "scale_degrees": [0, 2, 4, 5, 7]
                }
            },
            {
                "id": "adsr",
                "type": "adsr",
                "config": {
                    "attack": 0.01,
                    "decay": 0.1,
                    "sustain": 0.7,
                    "release": 0.2
                }
            },
            {
                "id": "osc",
                "type": "oscillator"
            },
            {
                "id": "vca",
                "type": "vca"
            },
            {
                "id": "dac",
                "type": "dac"
            }
        ],
        "connections": [
            {"from": "clock", "from_port": "gate", "to": "melody", "to_port": "gate"},
            {"from": "melody", "from_port": "gate", "to": "adsr", "to_port": "gate"},
            {"from": "melody", "from_port": "frequency", "to": "osc", "to_port": "frequency"},
            {"from": "adsr", "from_port": "envelope", "to": "vca", "to_port": "cv"},
            {"from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio"},
            {"from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio"}
        ]
    }
    "#;

    let invention: Invention = serde_json::from_str(json).expect("Failed to parse invention");
    let builder = InventionBuilder::new(44100);
    let (runtime, handles) = builder.build(invention).expect("Failed to build invention");
    let running = runtime
        .start_with_backend(NullAudioBackend::new(44100))
        .expect("Failed to start invention");

    // Clock feeds melody, which feeds both ADSR (gate) and oscillator (frequency)
    let tempo: ClockControls = handles.get("clock.controls").expect("No tempo handle");
    assert_eq!(tempo.get_bpm(), 120.0);
    running.stop();
}

/// Test unconnected inputs: Modules with some inputs unconnected should use defaults
#[test]
fn test_unconnected_inputs() {
    let json = r#"
    {
        "name": "Unconnected Inputs Test",
        "modules": [
            {
                "id": "clock",
                "type": "clock",
                "config": {"bpm": 120}
            },
            {
                "id": "osc",
                "type": "oscillator",
                "config": {"frequency": 440.0}
            },
            {
                "id": "vca",
                "type": "vca"
            },
            {
                "id": "dac",
                "type": "dac"
            }
        ],
        "connections": [
            {"from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio"},
            {"from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio"}
        ]
    }
    "#;

    let invention: Invention = serde_json::from_str(json).expect("Failed to parse invention");
    let builder = InventionBuilder::new(44100);
    let (runtime, _handles) = builder.build(invention).expect("Failed to build invention");
    let running = runtime
        .start_with_backend(NullAudioBackend::new(44100))
        .expect("Failed to start invention");

    // VCA cv input is unconnected - should default to 1.0 (passthrough)
    // Clock is also unconnected but present (required for runtime)
    // Should build successfully
    running.stop();
}

/// Test that cycles (feedback loops) are allowed and process safely.
///
/// Mutual FM between two oscillators is a common synthesis technique.
/// The pull-based engine handles this via one-sample feedback delay.
#[test]
fn test_cycle_is_safe() {
    let json = r#"
    {
        "name": "Cycle Test (feedback loop)",
        "modules": [
            {
                "id": "osc1",
                "type": "oscillator"
            },
            {
                "id": "osc2",
                "type": "oscillator"
            },
            {
                "id": "dac",
                "type": "dac"
            }
        ],
        "connections": [
            {"from": "osc1", "from_port": "audio", "to": "osc2", "to_port": "fm"},
            {"from": "osc2", "from_port": "audio", "to": "osc1", "to_port": "fm"},
            {"from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio"}
        ]
    }
    "#;

    let invention: Invention = serde_json::from_str(json).expect("Failed to parse invention");
    let builder = InventionBuilder::new(44100);
    let (runtime, _handles) = builder.build(invention).expect("Cycle should be allowed");
    let running = runtime
        .start_with_backend(NullAudioBackend::new(44100))
        .expect("Failed to start invention");

    // Let the audio thread process several samples with the feedback loop
    std::thread::sleep(std::time::Duration::from_millis(100));

    // If we get here without panic or hang, the cycle is safe
    running.stop();
}

/// Test complex valid graph: Multiple sources, converging paths
#[test]
fn test_complex_valid_graph() {
    let json = r#"
    {
        "name": "Complex Valid Graph",
        "modules": [
            {
                "id": "clock",
                "type": "clock",
                "config": {"bpm": 140}
            },
            {
                "id": "osc1",
                "type": "oscillator",
                "config": {"frequency": 110.0}
            },
            {
                "id": "osc2",
                "type": "oscillator",
                "config": {"frequency": 220.0}
            },
            {
                "id": "adsr",
                "type": "adsr",
                "config": {
                    "attack": 0.05,
                    "decay": 0.2,
                    "sustain": 0.5,
                    "release": 0.4
                }
            },
            {
                "id": "vca1",
                "type": "vca"
            },
            {
                "id": "vca2",
                "type": "vca"
            },
            {
                "id": "dac",
                "type": "dac"
            }
        ],
        "connections": [
            {"from": "clock", "from_port": "gate", "to": "adsr", "to_port": "gate"},
            {"from": "adsr", "from_port": "envelope", "to": "vca1", "to_port": "cv"},
            {"from": "adsr", "from_port": "envelope", "to": "vca2", "to_port": "cv"},
            {"from": "osc1", "from_port": "audio", "to": "vca1", "to_port": "audio"},
            {"from": "osc2", "from_port": "audio", "to": "vca2", "to_port": "audio"},
            {"from": "vca1", "from_port": "audio", "to": "dac", "to_port": "audio"},
            {"from": "vca2", "from_port": "audio", "to": "dac", "to_port": "audio"}
        ]
    }
    "#;

    let invention: Invention = serde_json::from_str(json).expect("Failed to parse invention");
    let builder = InventionBuilder::new(44100);
    let (runtime, handles) = builder.build(invention).expect("Failed to build invention");
    let running = runtime
        .start_with_backend(NullAudioBackend::new(44100))
        .expect("Failed to start invention");

    // Two separate voices with shared ADSR should work correctly
    let tempo: ClockControls = handles.get("clock.controls").expect("No tempo handle");
    assert_eq!(tempo.get_bpm(), 140.0);
    running.stop();
}
