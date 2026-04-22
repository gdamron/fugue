#[path = "../src/example_catalog.rs"]
mod example_catalog;

use example_catalog::{
    parse_example_command, playable_examples, resolve_example, usage, valid_examples_message,
    ExampleCommand, ExampleDefinition,
};
use fugue::{default_sample_rate, ClockControls, Invention, InventionBuilder};
use std::error::Error;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    let command = parse_example_command(std::env::args()).map_err(io::Error::other)?;

    match command {
        ExampleCommand::Help => {
            println!("{}", usage());
            println!();
            println!("{}", valid_examples_message());
            return Ok(());
        }
        ExampleCommand::Interactive => run_example(select_example_interactively()?)?,
        ExampleCommand::RunByName(file_name) => {
            run_example(resolve_example(&file_name).map_err(io::Error::other)?)?
        }
    }

    Ok(())
}

fn select_example_interactively() -> Result<ExampleDefinition, Box<dyn Error>> {
    let examples = playable_examples();
    if examples.is_empty() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::NotFound,
            "No playable examples were found in examples/.",
        )));
    }

    println!("=== Fugue Examples ===");
    println!();
    println!("Choose an invention to run:");
    for (index, example) in examples.iter().enumerate() {
        println!("  {}. {} ({})", index + 1, example.title, example.file_name);
        println!("     {}", example.summary);
    }
    println!();
    println!("Tip: run a specific file directly with:");
    println!("  cargo run --example examples -- --example <json-file>");
    println!();

    loop {
        print!("Select an example [1-{}]: ", examples.len());
        io::stdout().flush()?;

        let mut input = String::new();
        let bytes_read = io::stdin().read_line(&mut input)?;
        if bytes_read == 0 {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "No selection received from stdin.",
            )));
        }

        let trimmed = input.trim();
        let Ok(index) = trimmed.parse::<usize>() else {
            println!("Enter a number between 1 and {}.", examples.len());
            continue;
        };

        if let Some(example) = index.checked_sub(1).and_then(|index| examples.get(index)) {
            return Ok(*example);
        }

        println!("Enter a number between 1 and {}.", examples.len());
    }
}

fn run_example(example: ExampleDefinition) -> Result<(), Box<dyn Error>> {
    let sample_rate = default_sample_rate()?;
    let path = example.path();
    let invention =
        Invention::from_file(path.to_str().expect("example path should be valid UTF-8"))?;

    println!("=== Fugue Example Runner ===");
    println!();
    println!("Example: {}", example.title);
    println!("File: {}", path.display());
    println!(
        "Loaded invention: {}",
        invention.title.as_deref().unwrap_or("Untitled")
    );

    if let Some(description) = invention.description.as_deref() {
        if !description.is_empty() {
            println!("Description: {}", description);
        }
    }

    println!("Sample rate: {} Hz", sample_rate);
    println!();

    let builder = InventionBuilder::new(sample_rate);
    let (runtime, handles) = builder.build(invention)?;
    let tempo: Option<ClockControls> = handles.get("clock.controls");

    println!("Available handles:");
    for key in handles.keys() {
        println!("  - {}", key);
    }
    println!();

    let running = runtime.start()?;

    println!("Playback started.");
    if let Some(tempo) = tempo {
        println!("Tempo: {:.1} BPM", tempo.get_bpm());
    }
    println!("Press [Enter] to stop...");

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    println!("Stopping...");
    running.stop();
    thread::sleep(Duration::from_millis(100));
    println!("Done.");

    Ok(())
}
