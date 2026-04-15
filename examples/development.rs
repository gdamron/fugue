// Example: Development
//
// Demonstrates the use of developments, additional json files referenced within 
// an invention. This allows you to break up complex inventions into multiple files.
use fugue::{default_sample_rate, ClockControls, Invention, InventionBuilder};
use std::env;
use std::error::Error;
use std::io;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== Fugue Development Example ===");
    println!();
    println!("This example demonstrates the use of developments, which are additional");
    println!("JSON files that can be referenced within an invention. This allows you");
    println!("to break up complex inventions into multiple files for better organization.");
    println!();
    println!("args: --variant file|inline");
    println!();
    println!("file: Loads the development from an external JSON file (development_file_patch.json)");
    println!("inline: Loads the development from an inline definition within the invention (development_inline_patch.json)");
    println!();

    let args: Vec<String> = env::args().collect();
    let flag = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let variant = if flag == "--variant" {
        args.get(2).map(|s| s.as_str()).unwrap_or("file")
    } else {
        "file"
    };
    println!("Using variant: {}", variant);
    println!();

    // Get the audio device's sample rate BEFORE building the invention
    let sample_rate = default_sample_rate()?;

    // Load the invention
    let file = match variant {
        "file" => "examples/development_file_patch.json",
        "inline" => "examples/development_inline_patch.json",
        _ => {
            eprintln!("Unknown variant: {}. Use 'file' or 'inline'.", variant);
            return Ok(());
        }
    };
    let invention = Invention::from_file(file)?;
    println!(
        "Loaded invention: {}",
        invention.title.as_deref().unwrap_or("Untitled")
    );
    println!(
        "Description: {}",
        invention.description.as_deref().unwrap_or("")
    );
    println!();

    // Build and start the invention with the correct sample rate
    let builder = InventionBuilder::new(sample_rate);
    let (runtime, handles) = builder.build(invention)?;

    let tempo: ClockControls = handles
        .get("clock.controls")
        .expect("Invention should have a clock with controls handle");

    println!("Available handles:");
    for key in handles.keys() {
        println!("  - {}", key);
    }
    println!();

    let running = runtime.start()?;

    println!("Invention started!");
    println!();
    println!("Current settings:");
    println!("  Tempo: {:.1} BPM", tempo.get_bpm());
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
