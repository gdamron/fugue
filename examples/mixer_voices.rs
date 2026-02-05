// Example: Multi-Voice Mixer
//
// Demonstrates the mixer module combining multiple oscillators for a rich,
// layered sound. Three oscillators (saw, square, sine sub) are mixed together
// and processed through a filter with envelope modulation.
//
// Signal flow:
//                    ┌─> Osc (Saw) ────┐
//   Clock -> Melody ─┼─> Osc (Square) ─┼─> Mixer -> Filter -> VCA -> DAC
//              |     └─> Osc (Sine) ───┘      ^           ^
//              |                              |           |
//              └──────> Filter Env ───────────┘           |
//              └──────> Amp Env ──────────────────────────┘

use fugue::{default_sample_rate, ClockControls, Patch, PatchBuilder};
use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== Fugue Multi-Voice Mixer Example ===");
    println!();
    println!("Demonstrates mixing multiple oscillators for a layered sound.");
    println!();
    println!("Signal flow:");
    println!("                  ┌─> Osc (Saw) ────┐");
    println!("  Clock -> Melody ┼─> Osc (Square) ─┼─> Mixer -> Filter -> VCA -> DAC");
    println!("             |    └─> Osc (Sine) ───┘");
    println!("             |");
    println!("             └──> Envelopes (Filter + Amp)");
    println!();

    // Get the audio device's sample rate BEFORE building the patch
    let sample_rate = default_sample_rate()?;
    println!("Audio device sample rate: {} Hz", sample_rate);
    println!();

    // Load the patch
    let patch = Patch::from_file("examples/mixer_voices.json")?;
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
    println!("  Mixer levels: Saw 25%, Square 20%, Sub (Sine) 30%");
    println!("  Master level: 50%");
    println!("  Filter: Low-pass, 400 Hz base, 0.3 resonance");
    println!();
    println!("The three oscillators create a rich, full sound:");
    println!("  - Sawtooth: Bright, full harmonics");
    println!("  - Square: Hollow, adds body");
    println!("  - Sine (sub): Adds low-end weight");
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
