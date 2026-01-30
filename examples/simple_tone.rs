// Example: Simple 440Hz Tone with ADSR
//
// Minimal test to verify modular routing works:
// Clock (trigger) -> ADSR (envelope) -> VCA (cv)
// Oscillator (440Hz) -> VCA (audio) -> DAC
//
// Should produce a 440Hz tone that pulses on each beat with ADSR envelope shaping.

use fugue::{Patch, PatchBuilder, Tempo};
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

    // Load the simple patch
    let patch = Patch::from_file("examples/simple_tone.json")?;
    println!(
        "Loaded patch: {}",
        patch.title.as_deref().unwrap_or("Untitled")
    );
    println!();

    // Build the patch - returns both runtime and handles
    let builder = PatchBuilder::new(44100);
    let (runtime, handles) = builder.build(patch)?;

    // Get tempo handle for display
    let tempo: Tempo = handles
        .get("clock.tempo")
        .expect("Patch should have a clock with tempo handle");

    // Start the patch
    let running = runtime.start()?;

    println!("Patch started!");
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
