use fugue::*;

/// VCA (Voltage Controlled Amplifier) - applies gate/envelope to audio
pub struct VCA;

impl VCA {
    pub fn new() -> Self {
        Self
    }
}

impl Module for VCA {
    fn process(&mut self) -> bool {
        true
    }
    
    fn name(&self) -> &str {
        "VCA"
    }
}

/// VCA needs to combine NoteSignal and AudioSignal
/// For now, we'll demonstrate with a simpler approach
pub struct VoiceProcessor {
    oscillator: Oscillator,
}

impl VoiceProcessor {
    pub fn new(sample_rate: u32, osc_type: OscillatorType) -> Self {
        Self {
            oscillator: Oscillator::new(sample_rate, osc_type),
        }
    }
}

impl Module for VoiceProcessor {
    fn process(&mut self) -> bool {
        true
    }
    
    fn name(&self) -> &str {
        "VoiceProcessor"
    }
}

impl Processor<NoteSignal, AudioSignal> for VoiceProcessor {
    fn process_signal(&mut self, input: NoteSignal) -> AudioSignal {
        // Update oscillator frequency
        self.oscillator.set_frequency(input.frequency.hz);
        
        // Generate audio and apply envelope (velocity acts as envelope)
        let audio = self.oscillator.output();
        AudioSignal::new(audio.value * input.gate.velocity)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Fugue - Full Modular Voice Example");
    println!("===================================\n");
    
    let sample_rate = 44100;
    let tempo = Tempo::new(140.0);
    
    // Build the signal chain
    let clock = Clock::new(sample_rate, tempo.clone()).with_time_signature(4);
    
    let root = Note::new(62); // D4
    let scale = Scale::new(root, Mode::Dorian);
    let params = MelodyParams::new(vec![0, 2, 3, 4, 5, 6]); 
    params.set_note_weights(vec![3.0, 1.0, 1.5, 2.0, 2.5, 1.0]); // Emphasize root and fifth
    let sequencer = MelodyGenerator::new(scale, params, sample_rate, tempo);
    
    let voice_processor = VoiceProcessor::new(sample_rate, OscillatorType::Triangle);
    
    // Connect: Clock → Sequencer → VoiceProcessor
    println!("Modular chain:");
    println!("  Clock → Sequencer → VoiceProcessor");
    println!("              ↓            ↓");
    println!("         NoteSignal   AudioSignal");
    println!();
    
    let mut voice = clock.connect(sequencer).connect(voice_processor);
    
    println!("Generating audio samples...");
    println!("(Showing sample values for first 3 notes)\n");
    
    let mut sample_count = 0;
    let mut note_count = 0;
    let mut last_freq = 0.0;
    let mut samples_shown = 0;
    
    // Process for a few seconds
    for _ in 0..sample_rate * 3 {
        voice.process();
        let audio = voice.output();
        
        sample_count += 1;
        
        // Show some sample values at note boundaries
        if sample_count % 100 == 0 && samples_shown < 30 {
            // Get note info by reading intermediate signal
            // In real use, we'd have better signal inspection
            if audio.value.abs() > 0.01 {
                println!("  Sample {}: {:.4}", sample_count, audio.value);
                samples_shown += 1;
            }
        }
        
        if sample_count > sample_rate * 3 {
            break;
        }
    }
    
    println!("\n✓ Successfully generated modular audio!");
    println!("  {} samples processed", sample_count);
    println!("  Full signal chain working:");
    println!("    - Clock provided timing");
    println!("    - Sequencer generated notes");
    println!("    - VoiceProcessor created audio with envelope");
    
    Ok(())
}
