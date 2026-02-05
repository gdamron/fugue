// Example: LFO Tremolo with Sync
//
// Demonstrates synced LFO for rhythmic tremolo effect.
// Signal flow:
//   Clock -> MelodyGenerator -> Oscillator -> VCA -> DAC
//                |              ^
//                v              | (AM input)
//                └-> LFO ──────┘
//                    (sync)
//
// The LFO resets its phase on each new note, creating a consistent
// tremolo pattern that starts the same way for every note.

use fugue::{default_sample_rate, ClockControls, Patch, PatchBuilder};
use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== Fugue LFO Tremolo with Sync Example ===");
    println!();
    println!("This example demonstrates synced LFO for rhythmic tremolo.");
    println!();
    println!("Signal flow:");
    println!("  Clock -> MelodyGenerator -> Oscillator -> VCA -> DAC");
    println!("                |              ^");
    println!("                v              | (AM input)");
    println!("                └-> LFO ──────┘");
    println!("                    (sync)");
    println!();
    println!("The LFO resets on each note for consistent tremolo patterns.");
    println!();

    // Get the audio device's sample rate BEFORE building the patch
    let sample_rate = default_sample_rate()?;

    // Load the patch
    let patch = Patch::from_file("examples/lfo_tremolo_sync.json")?;
    println!(
        "Loaded patch: {}",
        patch.title.as_deref().unwrap_or("Untitled")
    );
    println!(
        "Description: {}",
        patch.description.as_deref().unwrap_or("")
    );
    println!();

    // Build and start the patch with the correct sample rate
    let builder = PatchBuilder::new(sample_rate);
    let (runtime, handles) = builder.build(patch)?;

    let tempo: ClockControls = handles
        .get("clock.controls")
        .expect("Patch should have a clock with controls handle");

    println!("Available handles:");
    for key in handles.keys() {
        println!("  - {}", key);
    }
    println!();

    let running = runtime.start()?;

    println!("Patch started!");
    println!();
    println!("Current settings:");
    println!("  Tempo: {:.1} BPM", tempo.get_bpm());
    println!("  LFO: 8 Hz triangle wave -> oscillator AM input");
    println!("  AM Amount: 0.6 (60% modulation depth)");
    println!("  Mode: Phrygian (exotic/Spanish character)");
    println!();
    println!("Listen for the rhythmic tremolo (amplitude pulsing) on each note.");
    println!("Each note starts with the same tremolo phase due to sync.");
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
