// Example: Filter Envelope Sweep
//
// Demonstrates classic subtractive synthesis with filter envelope modulation.
// This is the sound that defined analog synthesizers like the Minimoog.
//
// Signal flow:
//   Clock -> MelodyGenerator -> Oscillator -> Filter -> VCA -> DAC
//                |                            ^         ^
//                └─> Filter ADSR ─────────────┘         |
//                └─> Amp ADSR ──────────────────────────┘
//
// Two separate envelopes:
// - Filter envelope: Fast attack, medium decay - creates the "pluck" character
// - Amplitude envelope: Shapes the overall volume

use fugue::{Patch, PatchBuilder, Tempo};
use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== Fugue Filter Envelope Example ===");
    println!();
    println!("Classic subtractive synthesis with filter envelope modulation.");
    println!("This is the sound that defined analog synthesizers!");
    println!();
    println!("Signal flow:");
    println!("  Clock -> Melody -> Oscillator -> Filter -> VCA -> DAC");
    println!("              |                      ^         ^");
    println!("              └─> Filter Env ────────┘         |");
    println!("              └─> Amp Env ─────────────────────┘");
    println!();

    // Load the patch
    let patch = Patch::from_file("examples/filter_envelope.json")?;
    println!(
        "Loaded patch: {}",
        patch.title.as_deref().unwrap_or("Untitled")
    );
    println!(
        "Description: {}",
        patch.description.as_deref().unwrap_or("")
    );
    println!();

    // Build and start the patch
    let builder = PatchBuilder::new(44100);
    let (runtime, handles) = builder.build(patch)?;

    let tempo: Tempo = handles
        .get("clock.tempo")
        .expect("Patch should have a clock with tempo handle");

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
    println!("  Filter: Low-pass, 200 Hz base cutoff, 0.6 resonance");
    println!("  Filter Env: Fast attack (10ms), medium decay (400ms)");
    println!("  CV Amount: 4000 Hz (envelope sweeps cutoff up to ~4200 Hz)");
    println!();
    println!("Listen for the filter 'pluck' on each note - the bright attack");
    println!("that quickly decays to a darker, filtered tone.");
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
