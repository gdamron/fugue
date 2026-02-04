// Example: Step Sequencer Bass Line
//
// Demonstrates the step sequencer module with a classic bass pattern.
// Signal flow: Clock -> StepSequencer -> Oscillator -> VCA -> DAC
//                                    └-> ADSR ──────┘
//
// The step sequencer plays a deterministic 16-step pattern with per-step
// gate lengths. Unlike the MelodyGenerator which uses random selection,
// the step sequencer plays the exact same pattern every time.

use fugue::{default_sample_rate, Patch, PatchBuilder, Tempo};
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

    // Get the audio device's sample rate BEFORE building the patch
    let sample_rate = default_sample_rate()?;

    // Load the step sequencer patch
    let patch = Patch::from_file("examples/step_sequencer.json")?;
    println!(
        "Loaded patch: {}",
        patch.title.as_deref().unwrap_or("Untitled")
    );
    println!(
        "Description: {}",
        patch.description.as_deref().unwrap_or("")
    );
    println!();

    // Build the patch with the correct sample rate
    let builder = PatchBuilder::new(sample_rate);
    let (runtime, handles) = builder.build(patch)?;

    // Get the tempo handle for runtime control
    let tempo: Tempo = handles
        .get("clock.tempo")
        .expect("Patch should have a clock with tempo handle");

    println!("Available handles:");
    for key in handles.keys() {
        println!("  - {}", key);
    }
    println!();

    // Start the patch
    let running = runtime.start()?;

    println!("Patch started successfully!");
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
