// Verifies the Clock module emits correct gate pulses on its subdivision outputs.
//
// At 120 BPM / 44100 Hz, one beat = 22050 samples. Over exactly 4 beats we expect:
//   beat    — 4 rising edges
//   beat_d4 — 1 rising edge  (one every 4 beats)
//   beat_d2 — 2 rising edges (one every 2 beats)
//   beat_x2 — 8 rising edges (2 per beat)
//   beat_x4 — 16 rising edges (4 per beat)

#[cfg(test)]
mod tests {
    use fugue::*;

    const SUBDIVISIONS: [(&str, usize); 5] = [
        ("beat", 4),
        ("beat_d4", 1),
        ("beat_d2", 2),
        ("beat_x2", 8),
        ("beat_x4", 16),
    ];

    #[test]
    fn test_clock_subdivision_edge_counts() {
        let sample_rate = 44100;
        let mut clock = Clock::new(sample_rate, 120.0);

        let samples_per_beat = clock.samples_per_beat() as usize;
        let total_samples = samples_per_beat * 4;

        let mut prev = [0.0f32; SUBDIVISIONS.len()];
        let mut edges = [0usize; SUBDIVISIONS.len()];

        for _ in 0..total_samples {
            for (i, (port, _)) in SUBDIVISIONS.iter().enumerate() {
                let v = clock.get_output(port).unwrap();
                if prev[i] <= 0.5 && v > 0.5 {
                    edges[i] += 1;
                }
                prev[i] = v;
            }
            clock.process(1);
        }

        for (i, (port, expected)) in SUBDIVISIONS.iter().enumerate() {
            assert_eq!(
                edges[i], *expected,
                "{} should have {} rising edges in 4 beats, got {}",
                port, expected, edges[i]
            );
        }
    }

    #[test]
    fn test_clock_subdivision_unknown_port_errors() {
        let sample_rate = 44100;
        let clock = Clock::new(sample_rate, 120.0);
        assert!(clock.get_output("beat_x8").is_err());
        // The pre-convention names are gone (clean break).
        for old in ["gate", "gate_d4", "gate_d2", "gate_x2", "gate_x4"] {
            assert!(clock.get_output(old).is_err(), "{old}");
        }
    }

    #[test]
    fn test_clock_subdivision_duty_cycle_scales_with_period() {
        // With 50% gate_length, each subdivision port should be HIGH for
        // roughly half of its period — not half of a beat.
        let sample_rate = 44100;
        let mut clock = Clock::with_gate_length(sample_rate, 120.0, 0.5);

        let samples_per_beat = clock.samples_per_beat() as usize;
        let total_samples = samples_per_beat * 4;

        let ports = ["beat", "beat_d4", "beat_d2", "beat_x2", "beat_x4"];
        let mut high = [0usize; 5];

        for _ in 0..total_samples {
            for (i, port) in ports.iter().enumerate() {
                if clock.get_output(port).unwrap() > 0.5 {
                    high[i] += 1;
                }
            }
            clock.process(1);
        }

        // Every port emits a 50%-duty cycle, so regardless of period each one
        // is HIGH for ~half the window. Allow ±1% slack for integer truncation.
        let target = total_samples / 2;
        let slack = total_samples / 100;
        for (i, port) in ports.iter().enumerate() {
            let diff = high[i].abs_diff(target);
            assert!(
                diff <= slack,
                "{} HIGH samples {} should be ~{} (±{})",
                port,
                high[i],
                target,
                slack
            );
        }
    }
}

#[test]
fn the_clock_refuses_its_pre_convention_config_keys() {
    let registry = fugue::ModuleRegistry::default();
    let refused = [
        serde_json::json!({ "gate_duration": 0.5 }),
        serde_json::json!({ "time_signature": { "beats_per_measure": 4, "beat_unit": 4 } }),
    ];
    for config in refused {
        assert!(
            registry.build("clock", 48_000, &config).is_err(),
            "{config}"
        );
    }
    let config = serde_json::json!({ "bpm": 90.0, "gate_length": 0.5 });
    assert!(registry.build("clock", 48_000, &config).is_ok());
}
