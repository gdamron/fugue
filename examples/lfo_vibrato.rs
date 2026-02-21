// Example: LFO Vibrato
//
// Demonstrates LFO modulation for creating a vibrato effect.
// Signal flow:
//   Clock -> MelodyGenerator -> Oscillator -> VCA -> DAC
//                               ^
//                LFO (vibrato) -┘ (FM input)
//
// The LFO modulates the oscillator's pitch at ~5.5Hz, creating
// a subtle pitch wobble (vibrato) on each note.

use fugue::{default_sample_rate, ClockControls, Invention, InventionBuilder};
use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== Fugue LFO Vibrato Example ===");
    println!();
    println!("This example demonstrates LFO modulation for vibrato.");
    println!();
    println!("Signal flow:");
    println!("  Clock -> MelodyGenerator -> Oscillator -> VCA -> DAC");
    println!("                              ^");
    println!("               LFO (vibrato) -┘ (FM input)");
    println!();
    println!("The 5.5Hz sine LFO creates a subtle pitch wobble.");
    println!();

    // Get the audio device's sample rate BEFORE building the invention
    let sample_rate = default_sample_rate()?;

    // Load the invention
    let invention = Invention::from_file("examples/lfo_vibrato.json")?;
    println!(
        "Loaded invention: {}",
        invention.title.as_deref().unwrap_or("Untitled")
    );
    println!(
        "Description: {}",
        invention.description.as_deref().unwrap_or("")
    );
    println!();

    // Build and start the invention with the correct sample rate
    let builder = InventionBuilder::new(sample_rate);
    let (runtime, handles) = builder.build(invention)?;

    // Get tempo handle for display
    let tempo: ClockControls = handles
        .get("clock.controls")
        .expect("Invention should have a clock with controls handle");

    println!("Available handles:");
    for key in handles.keys() {
        println!("  - {}", key);
    }
    println!();

    let running = runtime.start()?;

    println!("Invention started!");
    println!();
    println!("Current settings:");
    println!("  Tempo: {:.1} BPM", tempo.get_bpm());
    println!("  LFO: 5.5 Hz sine wave -> oscillator FM input");
    println!("  FM Amount: 8 Hz deviation");
    println!();
    println!("Listen for the vibrato (pitch wobble) on each note.");
    println!();
    println!("Press [Enter] to quit...");

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    println!("Stopping...");
    running.stop();

    thread::sleep(Duration::from_millis(100));
    println!("Done!");

    Ok(())
}
