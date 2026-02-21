// Example: Simple 440Hz Tone with ADSR
//
// Minimal test to verify modular routing works:
// Clock (trigger) -> ADSR (envelope) -> VCA (cv)
// Oscillator (440Hz) -> VCA (audio) -> DAC
//
// Should produce a 440Hz tone that pulses on each beat with ADSR envelope shaping.

use fugue::{default_sample_rate, ClockControls, Invention, InventionBuilder};
use std::error::Error;
use std::io;

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== Simple 440Hz Tone Test ===");
    println!();
    println!("This is a minimal test of the modular routing system.");
    println!();
    println!("Signal flow:");
    println!("  Clock (trigger) -> ADSR (envelope)");
    println!("                          |");
    println!("  Oscillator (440Hz) -> VCA -> DAC");
    println!();

    // Get the audio device's sample rate BEFORE building the invention
    let sample_rate = default_sample_rate()?;

    // Load the simple invention
    let invention = Invention::from_file("examples/simple_tone.json")?;
    println!(
        "Loaded invention: {}",
        invention.title.as_deref().unwrap_or("Untitled")
    );
    println!();

    // Build the invention with the correct sample rate
    let builder = InventionBuilder::new(sample_rate);
    let (runtime, handles) = builder.build(invention)?;

    // Get tempo handle for display
    let tempo: ClockControls = handles
        .get("clock.controls")
        .expect("Invention should have a clock with controls handle");

    // Start the invention
    let running = runtime.start()?;

    println!("Invention started!");
    println!();
    println!(
        "You should hear a 440Hz tone pulsing at {:.0} BPM",
        tempo.get_bpm()
    );
    println!("with ADSR envelope shaping each pulse.");
    println!();
    println!("Press [Enter] to stop...");

    // Wait for user input
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    println!("Stopping...");
    running.stop();

    println!("Done!");
    Ok(())
}
