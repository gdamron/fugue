// Verifies the Clock module emits correct gate pulses on its subdivision outputs.
//
// At 120 BPM / 44100 Hz, one beat = 22050 samples. Over exactly 4 beats we expect:
//   gate    — 4 rising edges (beat)
//   gate_d4 — 1 rising edge  (whole note: ¼× beat rate)
//   gate_d2 — 2 rising edges (half note: ½× beat rate)
//   gate_x2 — 8 rising edges (8th note: 2× beat rate)
//   gate_x4 — 16 rising edges (16th note: 4× beat rate)

#[cfg(test)]
mod tests {
    use fugue::*;

    const SUBDIVISIONS: [(&str, usize); 5] = [
        ("gate", 4),
        ("gate_d4", 1),
        ("gate_d2", 2),
        ("gate_x2", 8),
        ("gate_x4", 16),
    ];

    #[test]
    fn test_clock_subdivision_edge_counts() {
        let sample_rate = 44100;
        let tempo = ClockControls::new(120.0);
        let mut clock = Clock::new(sample_rate, tempo.clone());

        let samples_per_beat = tempo.samples_per_beat(sample_rate) as usize;
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
            clock.process();
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
        let clock = Clock::new(sample_rate, ClockControls::new(120.0));
        assert!(clock.get_output("gate_x8").is_err());
    }

    #[test]
    fn test_clock_subdivision_duty_cycle_scales_with_period() {
        // With 50% gate_duration, each subdivision port should be HIGH for
        // roughly half of its period — not half of a beat.
        let sample_rate = 44100;
        let tempo = ClockControls::new_with_gate_duration(120.0, 0.5);
        let mut clock = Clock::new(sample_rate, tempo.clone());

        let samples_per_beat = tempo.samples_per_beat(sample_rate) as usize;
        let total_samples = samples_per_beat * 4;

        let ports = ["gate", "gate_d4", "gate_d2", "gate_x2", "gate_x4"];
        let mut high = [0usize; 5];

        for _ in 0..total_samples {
            for (i, port) in ports.iter().enumerate() {
                if clock.get_output(port).unwrap() > 0.5 {
                    high[i] += 1;
                }
            }
            clock.process();
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
