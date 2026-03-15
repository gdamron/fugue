# Fugue

A system for composing algorithmic and generative music.

## Features

- 🎵 **Cross-platform**: Designed to run everywhere
- 📄 **Inventions**: A declarative document format for describing a composition
- 🎚️ **Live control**: Update scales, rhythms, and synthesis parameters in real-time
- 🤖 **MCP Server**: Collaborate with LLM agents

## Quick Start

### Run the Examples

Fugue includes three examples demonstrating the modular routing system:

**1. Simple Tone** - Minimal working example
```bash
cargo run --example simple_tone
```
Demonstrates: Clock (PWM gate) → ADSR → VCA + Oscillator(440Hz) → DAC

### Run an Invention

Define your invention in a JSON file:

```json
{
  "version": "1.0.0",
  "title": "My Invention",
  "modules": [
    {
      "id": "clock",
      "type": "clock",
      "config": { "bpm": 120.0 }
    },
    {
      "id": "melody",
      "type": "melody",
      "config": {
        "root_note": 60,
        "mode": "dorian",
        "scale_degrees": [0, 1, 2, 3, 4, 5, 6]
      }
    },
    {
      "id": "voice",
      "type": "voice",
      "config": { "oscillator_type": "sine" }
    },
    {
      "id": "dac",
      "type": "dac",
      "config": {}
    }
  ],
  "connections": [
    { "from": "clock", "to": "melody" },
    { "from": "melody", "to": "voice" },
    { "from": "voice", "to": "dac" }
  ]
}
```

Load and run it:

```rust
use fugue::*;

let invention = Invention::from_file("my_invention.json")?;
let dac = Dac::new()?;
let builder = InventionBuilder::new(dac.sample_rate());
let runtime = builder.build_and_run(invention)?;
let running = runtime.start()?;

// Control parameters at runtime
running.tempo().set_bpm(140.0);
running.melody_params().set_note_weights(vec![1.0, 0.5, 1.0]);
```


## REPL (Interactive Terminal)

Fugue includes a terminal REPL for interactively building and tweaking inventions while audio plays.

### Build and Run

```bash
cargo run --features repl --bin fugue-repl
```

### Example Session

```
fugue> new My Jam
Created invention 'My Jam' with DAC.
fugue> add clock1 clock
Added clock 'clock1'.
fugue> add melody1 melody
Added melody 'melody1'.
fugue> add osc1 oscillator
Added oscillator 'osc1'.
fugue> connect clock1 gate melody1 gate
Connected clock1:gate -> melody1:gate
fugue> connect melody1 frequency osc1 frequency
Connected melody1:frequency -> osc1:frequency
fugue> connect osc1 audio dac audio
Connected osc1:audio -> dac:audio
fugue> set clock1 bpm 140
clock1.bpm = 140
fugue> controls clock1
clock1:
    bpm                 120  (60 - 300, default: 120) Tempo in beats per minute
fugue> types
(lists all module types with ports and controls)
fugue> quit
```

Type `help` for the full command reference.

## MCP Server (AI-Driven Composition)

Fugue includes an MCP server that exposes the full runtime API as tools, letting 
LLM agents like Claude create and manipulate inventions through natural 
conversation. Try starting with:

```plaintext
Create a new fugue invention.
```

### Setup

Build with the `mcp` feature:

```bash
cargo build --features mcp --bin fugue-mcp --release
```

### Use with Claude Desktop

Add to your Claude Desktop config (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):

```json
{
  "mcpServers": {
    "fugue": {
      "command": "/path/to/fugue/target/release/fugue-mcp"
    }
  }
}
```

### Use with Claude Code

Add to `.mcp.json` in the project root:

```json
{
  "mcpServers": {
    "fugue": {
      "command": "cargo",
      "args": ["run", "--features", "mcp", "--release", "--bin", "fugue-mcp"],
      "cwd": "/path/to/fugue"
    }
  }
}
```
