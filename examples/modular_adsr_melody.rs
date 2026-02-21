// Example: Modular ADSR Melody
//
// Demonstrates the modular routing system with named ports.
// Signal flow: Clock -> MelodyGenerator -> Oscillator -> VCA
//              Clock -> MelodyGenerator -> ADSR -> VCA -> DAC
//
// The ADSR envelope shapes the audio from the oscillator using a VCA,
// allowing for proper attack/decay/sustain/release control.

use fugue::{default_sample_rate, ClockControls, Invention, InventionBuilder};
use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== Fugue Modular ADSR Melody Example ===");
    println!();
    println!("This example demonstrates the modular routing system");
    println!("with named ports, enabling flexible signal routing.");
    println!();
    println!("Signal flow:");
    println!("  Clock -> MelodyGenerator -> Oscillator -> VCA -> DAC");
    println!("         └─────────────────-> ADSR ───────┘");
    println!();

    // Get the audio device's sample rate BEFORE building the invention
    let sample_rate = default_sample_rate()?;

    // Load the modular invention
    let invention = Invention::from_file("examples/modular_adsr_melody.json")?;
    println!(
        "Loaded invention: {}",
        invention.title.as_deref().unwrap_or("Untitled")
    );
    println!(
        "Description: {}",
        invention.description.as_deref().unwrap_or("")
    );
    println!();

    // Build the invention with the correct sample rate
    let builder = InventionBuilder::new(sample_rate);
    let (runtime, handles) = builder.build(invention)?;

    // Get the tempo handle for runtime control
    let tempo: ClockControls = handles
        .get("clock.controls")
        .expect("Invention should have a clock with controls handle");

    println!("Available handles:");
    for key in handles.keys() {
        println!("  - {}", key);
    }
    println!();

    // Start the invention
    let running = runtime.start()?;

    println!("Invention started successfully!");
    println!();
    println!("Controls:");
    println!("  [Enter] - Quit");
    println!();
    println!("Current settings:");
    println!("  Tempo: {:.1} BPM", tempo.get_bpm());
    println!();
    println!("Listening... (ADSR envelope shapes each note)");
    println!();

    // Wait for user input
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    println!("Stopping...");
    running.stop();

    // Give time for cleanup
    thread::sleep(Duration::from_millis(100));

    println!("Done!");
    Ok(())
}
