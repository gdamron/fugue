// Example: Step Sequencer Bass Line
//
// Demonstrates the step sequencer module with a classic bass pattern.
// Signal flow: Clock -> StepSequencer -> Oscillator -> VCA -> DAC
//                                    └-> ADSR ──────┘
//
// The step sequencer plays a deterministic 16-step pattern with per-step
// gate lengths. Unlike the MelodyGenerator which uses random selection,
// the step sequencer plays the exact same pattern every time.

use fugue::{default_sample_rate, ClockControls, Invention, InventionBuilder};
use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== Fugue Step Sequencer Example ===");
    println!();
    println!("This example demonstrates the step sequencer module");
    println!("playing a deterministic 16-step bass pattern.");
    println!();
    println!("Signal flow:");
    println!("  Clock -> StepSequencer -> Oscillator -> VCA -> DAC");
    println!("                        └-> ADSR ───────┘");
    println!();

    // Get the audio device's sample rate BEFORE building the invention
    let sample_rate = default_sample_rate()?;

    // Load the step sequencer invention
    let invention = Invention::from_file("examples/step_sequencer.json")?;
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
    println!("  Pattern: 16-step bass line in C");
    println!();
    println!("Listening... (step sequencer plays the same pattern repeatedly)");
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
