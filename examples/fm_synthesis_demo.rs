use fugue::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Fugue - FM Synthesis Example");
    println!("============================\n");

    // Load FM synthesis patch
    let patch = Patch::from_file("examples/fm_synthesis.json")?;

    println!(
        "Loaded patch: {}",
        patch.title.as_deref().unwrap_or("Untitled")
    );
    if let Some(desc) = &patch.description {
        println!("Description: {}", desc);
    }
    println!();

    // Build using oscillator patch builder
    let dac = Dac::new()?;
    let builder = OscillatorPatchBuilder::new(dac.sample_rate());
    let runtime = builder.build_and_run(patch)?;

    println!("Signal chain:");
    for module in runtime.patch().modules.iter() {
        println!("  [{}] {}", module.module_type, module.id);
    }
    println!();

    println!("Connections:");
    for conn in runtime.patch().connections.iter() {
        if let Some(port) = &conn.to_port {
            println!("  {} -> {} (port: {})", conn.from, conn.to, port);
        } else {
            println!("  {} -> {}", conn.from, conn.to);
        }
    }
    println!();

    // Start audio
    let running = runtime.start()?;

    println!("Audio started! 🔊");
    println!("You should hear FM synthesis (modulator affecting carrier frequency)");
    println!();
    println!("Press Enter to stop...");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    running.stop();
    println!("Stopped.");

    Ok(())
}
