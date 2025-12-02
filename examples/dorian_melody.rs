use fugue::*;
use std::io::{self, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Fugue - Algorithmic Music Composition (Modular)");
    println!("================================================\n");

    let sample_rate = 44100;
    let tempo = Tempo::new(120.0);
    
    let root = Note::new(60); // Middle C
    let scale = Scale::new(root, Mode::Dorian);
    
    let allowed_degrees = vec![0, 1, 2, 3, 4, 5, 6];
    let params = MelodyParams::new(allowed_degrees);
    
    // Build modular chain: Clock → Sequencer
    let clock = Clock::new(sample_rate, tempo.clone()).with_time_signature(4);
    let sequencer = MelodyGenerator::new(scale, params.clone(), sample_rate, tempo.clone());
    
    let voice = clock.connect(sequencer);
    
    let mut engine = ModularAudioEngine::new()?;
    
    println!("Starting Dorian melody at 120 BPM...");
    println!("Sample rate: {} Hz", engine.sample_rate());
    println!("\nModular chain: Clock → Sequencer → Oscillator → Audio\n");
    
    engine.start_voice(voice, params.clone())?;
    
    println!("Commands:");
    println!("  1-7: Toggle scale degrees (1=root, 2=second, etc.)");
    println!("  s/w/t/q: Change oscillator (Sine/saWtooth/Triangle/sQuare)");
    println!("  +/-: Increase/decrease tempo");
    println!("  f/n: Faster/slower notes");
    println!("  r: Emphasize root and fifth");
    println!("  x: Exit\n");

    let mut current_degrees = vec![0, 1, 2, 3, 4, 5, 6];
    
    loop {
        print!("> ");
        io::stdout().flush()?;
        
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        
        match input.trim() {
            "1" => toggle_degree(&mut current_degrees, &params, 0),
            "2" => toggle_degree(&mut current_degrees, &params, 1),
            "3" => toggle_degree(&mut current_degrees, &params, 2),
            "4" => toggle_degree(&mut current_degrees, &params, 3),
            "5" => toggle_degree(&mut current_degrees, &params, 4),
            "6" => toggle_degree(&mut current_degrees, &params, 5),
            "7" => toggle_degree(&mut current_degrees, &params, 6),
            "s" => {
                params.set_oscillator_type(OscillatorType::Sine);
                println!("✓ Switched to Sine wave");
            }
            "w" => {
                params.set_oscillator_type(OscillatorType::Sawtooth);
                println!("✓ Switched to Sawtooth wave");
            }
            "t" => {
                params.set_oscillator_type(OscillatorType::Triangle);
                println!("✓ Switched to Triangle wave");
            }
            "q" => {
                params.set_oscillator_type(OscillatorType::Square);
                println!("✓ Switched to Square wave");
            }
            "+" => {
                let new_tempo = tempo.get_bpm() + 10.0;
                tempo.set_bpm(new_tempo);
                println!("✓ Tempo: {} BPM", new_tempo);
            }
            "-" => {
                let new_tempo = (tempo.get_bpm() - 10.0).max(40.0);
                tempo.set_bpm(new_tempo);
                println!("✓ Tempo: {} BPM", new_tempo);
            }
            "f" => {
                let new_duration = (*params.note_duration.lock().unwrap() * 0.75).max(0.0625);
                params.set_note_duration(new_duration);
                println!("✓ Note duration: {:.3} beats", new_duration);
            }
            "n" => {
                let new_duration = (*params.note_duration.lock().unwrap() * 1.33).min(2.0);
                params.set_note_duration(new_duration);
                println!("✓ Note duration: {:.3} beats", new_duration);
            }
            "r" => {
                let weights = vec![3.0, 1.0, 1.0, 1.0, 2.5, 1.0, 1.0];
                params.set_note_weights(weights);
                println!("✓ Emphasized root and fifth");
            }
            "x" => {
                println!("Stopping...");
                break;
            }
            "" => continue,
            _ => println!("Unknown command. Type 'x' to exit."),
        }
    }
    
    engine.stop();
    println!("Goodbye!");
    
    Ok(())
}

fn toggle_degree(current: &mut Vec<usize>, params: &MelodyParams, degree: usize) {
    if let Some(pos) = current.iter().position(|&d| d == degree) {
        current.remove(pos);
        println!("✗ Removed degree {} ({})", degree + 1, degree_name(degree));
    } else {
        current.push(degree);
        current.sort();
        println!("✓ Added degree {} ({})", degree + 1, degree_name(degree));
    }
    
    if current.is_empty() {
        println!("⚠ Warning: No degrees active, re-adding all");
        *current = vec![0, 1, 2, 3, 4, 5, 6];
    }
    
    params.set_allowed_degrees(current.clone());
    println!("  Active degrees: {:?}", current.iter().map(|d| d + 1).collect::<Vec<_>>());
}

fn degree_name(degree: usize) -> &'static str {
    match degree {
        0 => "root",
        1 => "2nd",
        2 => "♭3rd",
        3 => "4th",
        4 => "5th",
        5 => "6th",
        6 => "♭7th",
        _ => "?",
    }
}
