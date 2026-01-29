// Example: Modular ADSR Melody
//
// Demonstrates the new modular routing system with named ports.
// Signal flow: Clock → MelodyGenerator → Oscillator → VCA
//              Clock → MelodyGenerator → ADSR → VCA → DAC
//
// The ADSR envelope shapes the audio from the oscillator using a VCA,
// allowing for proper attack/decay/sustain/release control.

use fugue::{ModularPatchBuilder, Patch};
use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== Fugue Modular ADSR Melody Example ===");
    println!();
    println!("This example demonstrates the new modular routing system");
    println!("with named ports, enabling flexible signal routing.");
    println!();
    println!("Signal flow:");
    println!("  Clock → MelodyGenerator → Oscillator → VCA → DAC");
    println!("         └─────────────────→ ADSR ───────┘");
    println!();

    // Load the modular patch
    let patch = Patch::from_file("examples/modular_adsr_melody.json")?;
    println!(
        "Loaded patch: {}",
        patch.title.as_deref().unwrap_or("Untitled")
    );
    println!(
        "Description: {}",
        patch.description.as_deref().unwrap_or("")
    );
    println!();

    // Build and start the modular patch
    let builder = ModularPatchBuilder::new(44100);
    let runtime = builder.build(patch)?;
    let running = runtime.start()?;

    println!("✓ Patch started successfully!");
    println!();
    println!("Controls:");
    println!("  [Enter] - Quit");
    println!();
    println!("Current settings:");
    println!("  Tempo: {:.1} BPM", running.tempo().get_bpm());
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
