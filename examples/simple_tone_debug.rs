// Example: Simple 440Hz Tone with ADSR - DEBUG VERSION
//
// This version prints signal values to verify the audio chain is working correctly.

use fugue::{ModularPatchBuilder, Patch};
use std::error::Error;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== Simple 440Hz Tone Test (DEBUG) ===");
    println!();
    println!("Signal flow:");
    println!("  Clock (trigger) → ADSR (envelope)");
    println!("                          ↓");
    println!("  Oscillator (440Hz) → VCA → DAC");
    println!();

    // Load the simple patch
    let patch = Patch::from_file("examples/simple_tone.json")?;
    println!(
        "Loaded patch: {}",
        patch.title.as_deref().unwrap_or("Untitled")
    );
    println!();

    // Build and start
    let builder = ModularPatchBuilder::new(44100);
    let runtime = builder.build(patch)?;

    // Access the modules to print their state
    println!("Modules created:");
    for (id, _module) in runtime.modules().iter() {
        println!("  - {}", id);
    }
    println!();

    let running = runtime.start()?;

    println!("✓ Patch started!");
    println!();
    println!("Monitoring signal flow for 5 seconds...");
    println!("(Should see triggers at ~120 BPM = every ~0.5 seconds)");
    println!();

    // Run for 5 seconds and monitor
    for i in 0..10 {
        thread::sleep(Duration::from_millis(500));
        println!("[{:.1}s] Still running...", i as f32 * 0.5);
    }

    println!();
    println!("Stopping...");
    running.stop();

    println!("Done!");
    Ok(())
}
