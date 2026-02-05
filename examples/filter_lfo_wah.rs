// Example: Filter LFO Wah
//
// Demonstrates LFO modulation of filter cutoff for a wah-wah/auto-filter effect.
// The triangle LFO sweeps the filter cutoff up and down continuously.
//
// Signal flow:
//   Clock -> MelodyGenerator -> Oscillator -> Filter -> VCA -> DAC
//                               ^             ^         ^
//                               |    LFO ─────┘         |
//                               |                       |
//                               └───────────────ADSR ───┘

use fugue::{default_sample_rate, ClockControls, Patch, PatchBuilder};
use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== Fugue Filter LFO Wah Example ===");
    println!();
    println!("LFO modulation of filter cutoff for wah-wah effect.");
    println!();
    println!("Signal flow:");
    println!("  Clock -> Melody -> Oscillator -> Filter -> VCA -> DAC");
    println!("                                   ^");
    println!("                          LFO ─────┘ (cutoff modulation)");
    println!();

    // Get the audio device's sample rate BEFORE building the patch
    let sample_rate = default_sample_rate()?;

    // Load the patch
    let patch = Patch::from_file("examples/filter_lfo_wah.json")?;
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
    println!("  Filter: Low-pass, 800 Hz base, 0.7 resonance");
    println!("  LFO: 2 Hz triangle wave (using unipolar output)");
    println!("  CV Amount: 2000 Hz (sweeps between 800-2800 Hz)");
    println!();
    println!("Listen for the rhythmic 'wah' effect as the filter sweeps.");
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
