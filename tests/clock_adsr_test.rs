// Test to debug Clock → ADSR gate flow (PWM)

#[cfg(test)]
mod tests {
    use fugue::*;

    #[test]
    fn test_clock_pwm_gate() {
        let sample_rate = 44100;
        let tempo = ClockControls::new(120.0);
        let mut clock = Clock::new(sample_rate, tempo.clone());
        let mut adsr = Adsr::new(sample_rate);

        // At 120 BPM, one beat = 0.5 seconds = 22050 samples
        let samples_per_beat = tempo.samples_per_beat(sample_rate) as usize;

        println!("Testing Clock PWM Gate → ADSR");
        println!("Tempo: 120 BPM = {} samples/beat", samples_per_beat);
        println!("Gate duration: 50% = {} samples HIGH", samples_per_beat / 2);
        println!();

        let mut gate_high_count = 0;
        let mut max_envelope = 0.0f32;

        // Run for 2 beats
        let mut last_gate = 0.0f32;
        for i in 0..(samples_per_beat * 2) {
            let gate = clock.get_output("gate").unwrap();

            if gate > 0.5 {
                gate_high_count += 1;
            }

            // Route to ADSR
            adsr.set_input("gate", gate).unwrap();
            adsr.process();
            let envelope = adsr.get_output("envelope").unwrap();

            max_envelope = max_envelope.max(envelope);

            // Print key events (gate rising edge)
            if gate > 0.5 && last_gate <= 0.5 {
                println!(
                    "Sample {}: GATE ON! gate={:.1}, envelope={:.6}",
                    i, gate, envelope
                );
            }

            if i % 5000 == 0 {
                println!("Sample {}: gate={:.1}, envelope={:.6}", i, gate, envelope);
            }

            last_gate = gate;

            // Advance clock
            clock.process();
        }

        println!();
        println!(
            "Gate was HIGH for {} samples out of {}",
            gate_high_count,
            samples_per_beat * 2
        );
        println!("Expected: ~{} samples (50% duty cycle)", samples_per_beat);
        println!("Max envelope reached: {:.6}", max_envelope);

        assert!(
            gate_high_count > samples_per_beat / 4,
            "Gate should be HIGH for significant time"
        );
        assert!(
            max_envelope > 0.1,
            "Envelope should reach significant amplitude"
        );
    }
}
