# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build and Run Commands

```bash
# Build the library
cargo build

# Run the main example (Dorian melody with live controls)
cargo run --example dorian_melody

# Run other examples
cargo run --example modular_clock
cargo run --example modular_chain
cargo run --example modular_voice

# Build the MCP server (requires mcp feature)
cargo build --features mcp --bin fugue-mcp

# Run the MCP server
cargo run --features mcp --bin fugue-mcp

# Run tests
cargo test

# Check for compile errors without building
cargo check
```

## Architecture Overview

Fugue is a Rust library for algorithmic/generative music composition using a modular synthesis approach inspired by Eurorack, ChucK, and WebAudio.

### Signal Philosophy

**All signals are raw `f32` values** - like voltages in real modular synthesizers. Port names determine how modules interpret values:

- **`"audio"`** - Audio-rate waveforms
- **`"gate"`** - Trigger signals (1.0 = on, 0.0 = off)
- **`"frequency"`** - Pitch in Hz
- **`"envelope"`** - Amplitude envelope (0.0-1.0)
- **`"cv"`** - Control voltage for modulation
- **`"fm"`** - Frequency modulation input
- **`"am"`** - Amplitude modulation input

This uniform approach enables flexible routing: any output can connect to any compatible input.

### Module System

All components implement the `Module` trait from `src/traits.rs`:

```rust
pub trait Module: Send {
    fn name(&self) -> &str;
    fn process(&mut self) -> bool;
    fn inputs(&self) -> &[&str];
    fn outputs(&self) -> &[&str];
    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String>;
    fn get_output(&self, port: &str) -> Result<f32, String>;
    fn last_processed_sample(&self) -> u64;
    fn mark_processed(&mut self, sample: u64);
}
```

Modules declare explicit port names and connect via named ports in JSON inventions:
```json
{
  "connections": [
    {"from": "adsr", "from_port": "envelope", "to": "vca", "to_port": "cv"},
    {"from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio"}
  ]
}
```

### Core Modules

| Module | Location | Purpose | Key Ports |
|--------|----------|---------|-----------|
| `Clock` | `modules/clock/` | Tempo-driven timing | out: `gate` |
| `Tempo` | `modules/clock/tempo.rs` | Thread-safe BPM control | (shared state) |
| `Oscillator` | `modules/oscillator/` | Waveform generation | in: `frequency`, `fm`, `am`; out: `audio` |
| `MelodyGenerator` | `modules/melody/` | Algorithmic note sequencing | in: `gate`; out: `frequency`, `gate` |
| `Adsr` | `modules/adsr/` | ADSR envelope generator | in: `gate`; out: `envelope` |
| `Vca` | `modules/vca/` | Voltage-controlled amplifier | in: `audio`, `cv`; out: `audio` |
| `Dac` | `modules/dac/` | Audio output via cpal | in: `audio` (from closure) |
| `Scale`/`Mode`/`Note` | `music/` | Music theory utilities | (data structures) |

### Pull-Based Signal Processing

The system uses **pull-based processing** where the DAC recursively requests inputs from connected modules:

1. DAC requests its inputs for the current sample
2. Each module recursively pulls from its dependencies (depth-first traversal)
3. Modules cache their outputs per sample to avoid reprocessing
4. Natural dependency resolution ensures correct processing order

**Typical signal flow**:
```
Clock (gate) → MelodyGenerator (frequency+gate) → Oscillator (audio) → Dac
                                                        ↓
                                                    Adsr (envelope) → Vca → Dac
```

### Thread Safety Pattern

Shared state uses `Arc<Mutex<T>>` for lock-free-ish updates between main/audio threads. Example:
```rust
tempo.set_bpm(140.0);  // Main thread
let bpm = tempo.get_bpm();  // Audio thread reads latest value
```

## Declarative Invention System

Fugue supports both declarative (JSON) and programmatic (Rust) approaches.

### Load and Run a JSON Invention

```rust
let invention = Invention::from_file("my_invention.json")?;
let dac = Dac::new()?;
let builder = InventionBuilder::new(dac.sample_rate());
let runtime = builder.build_and_run(invention)?;
let running = runtime.start()?;

// Control at runtime
running.tempo().set_bpm(140.0);
```

### Supported Module Types

- **clock** - Timing/tempo
- **melody** - Algorithmic melody generation
- **voice** - Note-to-audio with oscillator
- **oscillator** - Standalone for FM/AM synthesis
- **dac** - Audio output

### FM/AM Synthesis with Named Ports

```json
{"from": "modulator", "to": "carrier", "to_port": "fm"}
{"from": "lfo", "to": "carrier", "to_port": "am"}
```

### Multiple Parallel Voices

Multiple paths automatically mix at the DAC:
```json
{
  "connections": [
    {"from": "melody1", "from_port": "frequency", "to": "voice1", "to_port": "frequency"},
    {"from": "melody2", "from_port": "frequency", "to": "voice2", "to_port": "frequency"},
    {"from": "voice1", "from_port": "audio", "to": "dac", "to_port": "audio"},
    {"from": "voice2", "from_port": "audio", "to": "dac", "to_port": "audio"}
  ]
}
```

## MCP Server

Fugue includes an MCP (Model Context Protocol) server that lets LLM agents create and manipulate inventions in real time. The server is built with `rmcp` and communicates over stdio.

### Building

The MCP server is gated behind the `mcp` cargo feature to keep the core library lean:

```bash
cargo build --features mcp --bin fugue-mcp
```

### Claude Desktop Configuration

Add to `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "fugue": {
      "command": "cargo",
      "args": ["run", "--features", "mcp", "--bin", "fugue-mcp"],
      "cwd": "/path/to/fugue"
    }
  }
}
```

Or with a pre-built binary:

```json
{
  "mcpServers": {
    "fugue": {
      "command": "/path/to/fugue/target/release/fugue-mcp"
    }
  }
}
```

### Claude Code Configuration

Add to `.mcp.json` in the project root or `~/.claude/mcp.json` globally:

```json
{
  "mcpServers": {
    "fugue": {
      "command": "cargo",
      "args": ["run", "--features", "mcp", "--bin", "fugue-mcp"],
      "cwd": "/path/to/fugue"
    }
  }
}
```

### Available Tools (14)

**Lifecycle:**
- `create_invention(title?)` — Creates a minimal invention (DAC only) and starts playback
- `load_invention(json)` — Loads a full invention from a JSON string
- `stop_invention()` — Stops playback
- `get_status()` — Returns running state, module count, connection count

**Modules:**
- `add_module(id, module_type, config?)` — Adds a module to the running invention
- `remove_module(id)` — Removes a module and its connections
- `list_modules()` — Lists all modules with their types

**Connections:**
- `connect(from, from_port, to, to_port)` — Connects two module ports
- `disconnect(from, from_port, to, to_port)` — Removes a connection
- `list_connections()` — Lists all connections

**Controls:**
- `set_control(module_id, key, value)` — Sets a control value (e.g. BPM, attack time)
- `get_control(module_id, key)` — Reads the current value of a control
- `list_controls(module_id?)` — Lists controls with descriptions, ranges, and defaults

**Discovery:**
- `describe_module_types()` — Lists all module types with their ports and controls. Call this first to understand available modules.

### Typical LLM Workflow

1. Call `describe_module_types` to discover available modules, ports, and controls
2. Call `create_invention` to start with a blank canvas (DAC only)
3. Add modules: `add_module("clock1", "clock")`, `add_module("osc1", "oscillator")`, etc.
4. Wire them up: `connect("clock1", "gate", "melody1", "gate")`
5. Adjust parameters: `set_control("clock1", "bpm", 140.0)`
6. Iterate — add/remove modules, change connections and controls in real time
