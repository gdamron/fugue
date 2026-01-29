// Unit test to verify VCA passthrough behavior

#[cfg(test)]
mod tests {
    use fugue::*;

    #[test]
    fn test_vca_passthrough() {
        // Create modules
        let sample_rate = 44100;
        let mut osc = Oscillator::new(sample_rate, OscillatorType::Sine);
        osc.set_frequency(440.0);
        let mut vca = Vca::new();

        println!("Testing VCA passthrough (no CV connected):");
        println!();

        // Simulate a few samples
        for i in 0..5 {
            // Process oscillator
            osc.process();
            let osc_audio = osc.get_output("audio").unwrap();

            // Route to VCA (no CV connection)
            vca.set_input("audio", osc_audio).unwrap();
            let vca_output = vca.get_output("audio").unwrap();

            println!(
                "Sample {}: osc={:.6}, vca={:.6}, ratio={:.2}",
                i,
                osc_audio,
                vca_output,
                if osc_audio.abs() > 0.001 {
                    vca_output / osc_audio
                } else {
                    0.0
                }
            );

            // VCA should pass audio through unchanged (CV defaults to 1.0)
            assert!(
                (vca_output - osc_audio).abs() < 0.0001,
                "VCA should passthrough when CV=1.0"
            );
        }

        println!();
        println!("✓ VCA passthrough works correctly!");
    }

    #[test]
    fn test_vca_with_envelope() {
        // Create modules
        let sample_rate = 44100;
        let mut clock = Clock::new(sample_rate, Tempo::new(120.0));
        let mut adsr = Adsr::new(sample_rate);
        let mut osc = Oscillator::new(sample_rate, OscillatorType::Sine);
        osc.set_frequency(440.0);
        let mut vca = Vca::new();

        println!("Testing VCA with ADSR envelope:");
        println!();

        // Trigger the envelope
        let gate = clock.get_output("gate").unwrap();
        adsr.set_input("gate", gate).unwrap();
        adsr.process();

        let envelope = adsr.get_output("envelope").unwrap();
        println!("Initial envelope value: {:.6}", envelope);
        println!();

        // Process a few samples
        for i in 0..5 {
            clock.process();
            osc.process();

            let gate = clock.get_output("gate").unwrap();
            let osc_audio = osc.get_output("audio").unwrap();

            adsr.set_input("gate", gate).unwrap();
            adsr.process();
            let envelope = adsr.get_output("envelope").unwrap();

            vca.set_input("audio", osc_audio).unwrap();
            vca.set_input("cv", envelope).unwrap();
            let vca_output = vca.get_output("audio").unwrap();

            println!(
                "Sample {}: osc={:.6}, env={:.6}, vca={:.6}",
                i, osc_audio, envelope, vca_output
            );

            // VCA output should equal osc * envelope
            let expected = osc_audio * envelope;
            assert!(
                (vca_output - expected).abs() < 0.0001,
                "VCA output should equal audio * cv"
            );
        }

        println!();
        println!("✓ VCA envelope control works correctly!");
    }
}
