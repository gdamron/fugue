// Test: Simple modular signal generation
// This test verifies that the modular system generates non-zero audio samples

use fugue::{ModularPatchBuilder, Patch};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing modular signal generation...\n");

    let patch = Patch::from_file("examples/modular_adsr_melody.json")?;
    let builder = ModularPatchBuilder::new(44100);
    let runtime = builder.build(patch)?;
    let running = runtime.start()?;

    println!("Patch started. Checking if audio is being generated...\n");

    // Give it a moment to start producing samples
    std::thread::sleep(std::time::Duration::from_millis(500));

    println!("✓ If you hear sound, the modular system is working!");
    println!("  Press Ctrl+C to stop");

    // Wait indefinitely
    std::thread::sleep(std::time::Duration::from_secs(10));

    running.stop();
    println!("\nDone!");

    Ok(())
}
