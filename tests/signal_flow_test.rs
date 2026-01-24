// Unit test to verify modular signal flow

#[cfg(test)]
mod tests {
    use fugue::*;

    #[test]
    fn test_simple_signal_flow() {
        // Create modules
        let sample_rate = 44100;
        let mut clock = time::Clock::new(sample_rate, time::Tempo::new(120.0));
        let mut adsr = synthesis::ModularAdsr::new(sample_rate);
        let mut osc = Oscillator::new(sample_rate, OscillatorType::Sine);
        osc.set_frequency(440.0);
        let mut vca = synthesis::Vca::new();

        println!("Initial state:");
        println!("  Clock samples_elapsed: {}", clock.samples_elapsed());
        println!(
            "  Clock trigger (before first process): {:.2}",
            clock.get_output("trigger").unwrap()
        );
        println!();

        // Simulate a few samples
        for i in 0..10 {
            // Get outputs for THIS sample (before processing)
            let trigger = clock.get_output("trigger").unwrap();
            let beat = clock.get_output("beat").unwrap();
            let osc_audio = osc.get_output("audio").unwrap();

            // Route signals
            adsr.set_input("gate", trigger).unwrap();
            adsr.process();

            let envelope = adsr.get_output("envelope").unwrap();

            vca.set_input("audio", osc_audio).unwrap();
            vca.set_input("cv", envelope).unwrap();

            let output = vca.get_output("audio").unwrap();

            println!(
                "Sample {}: beat={:.4}, trigger={:.2}, envelope={:.4}, osc={:.4}, vca={:.4}",
                i, beat, trigger, envelope, osc_audio, output
            );

            // Process modules to advance to NEXT sample
            clock.process();
            osc.process();
        }
    }
}
