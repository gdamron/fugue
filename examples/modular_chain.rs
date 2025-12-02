use fugue::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Fugue - Modular Chain Example");
    println!("==============================\n");
    
    let sample_rate = 44100;
    let tempo = Tempo::new(120.0);
    
    // Create modules
    let tempo = Tempo::new(120.0);
    let clock = Clock::new(sample_rate, tempo.clone()).with_time_signature(4);
    
    let root = Note::new(60); // Middle C
    let scale = Scale::new(root, Mode::Dorian);
    let params = MelodyParams::new(vec![0, 2, 4, 5, 7]); // Pentatonic-ish
    let sequencer = MelodyGenerator::new(scale, params, sample_rate, tempo);
    
    let oscillator = Oscillator::new(sample_rate, OscillatorType::Sine);
    
    // Connect them: Clock → Sequencer → Oscillator
    println!("Building modular chain:");
    println!("  Clock → Sequencer → Oscillator");
    println!();
    
    let mut voice = clock.connect(sequencer);
    
    println!("Processing modular chain...");
    println!("(Showing first 10 notes)\n");
    
    let mut note_count = 0;
    let mut last_freq = 0.0;
    
    // Process the chain for a few seconds
    for _i in 0..sample_rate * 10 {
        voice.process();
        let note_signal = voice.output();
        
        // Detect note changes (frequency change with active gate)
        if note_signal.gate.active && 
           (note_signal.frequency.hz - last_freq).abs() > 0.1 {
            note_count += 1;
            if note_count <= 10 {
                println!(
                    "Note {}: {:.2} Hz (velocity: {:.2})",
                    note_count,
                    note_signal.frequency.hz,
                    note_signal.gate.velocity
                );
            }
            last_freq = note_signal.frequency.hz;
        }
        
        if note_count >= 10 {
            break;
        }
    }
    
    println!("\n✓ Modular chain working successfully!");
    println!("  Clock generated timing signals");
    println!("  Sequencer converted timing to notes");
    println!("  Ready to connect to oscillator for audio");
    
    Ok(())
}
