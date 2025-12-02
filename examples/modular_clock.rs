use fugue::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Fugue Modular Clock Example");
    println!("===========================\n");

    // Create a master clock - a pure generator module
    let sample_rate = 44100;
    let tempo = Tempo::new(120.0);
    let mut clock = Clock::new(sample_rate, tempo)
        .with_time_signature(4); // 4/4 time

    println!("Clock initialized:");
    println!("  Sample rate: {} Hz", sample_rate);
    println!("  Tempo: {} BPM", clock.tempo().get_bpm());
    println!("  Time signature: 4/4\n");

    println!("Simulating clock ticks...\n");
    
    // Simulate some time passing
    let mut last_beat = 0;
    let mut last_measure = 0;
    
    for _ in 0..sample_rate * 5 {  // 5 seconds
        // Process the clock (advances time)
        clock.process();
        
        // Read the clock signal
        let signal = clock.output();
        
        // Print on beat boundaries
        let current_beat = signal.beats.floor() as u64;
        if current_beat != last_beat {
            println!(
                "Beat {} | Measure {} | Beat in measure {} | Phase: {:.3}",
                current_beat,
                signal.measure,
                signal.beat_in_measure,
                signal.phase
            );
            last_beat = current_beat;
        }
        
        // Print on measure boundaries
        if signal.measure != last_measure {
            println!("  ▶ New measure: {}", signal.measure);
            last_measure = signal.measure;
        }
    }
    
    println!("\nClock demonstration complete!");
    println!("Total beats elapsed: {:.2}", clock.beats_elapsed());
    println!("Time elapsed: {:.2}s", clock.time_elapsed().as_secs_f64());

    Ok(())
}
