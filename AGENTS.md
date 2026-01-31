# AGENTS.md

This file provides guidance to agentic coding assistants working in the Fugue repository.

## Build and Test Commands

### Building
```bash
# Build the library
cargo build

# Build in release mode (optimized)
cargo build --release

# Check for compile errors without building
cargo check
```

### Testing
```bash
# Run all tests
cargo test

# Run a single test by name
cargo test test_name

# Run tests matching a pattern
cargo test pattern_name

# Run tests with output visible
cargo test -- --nocapture

# Run tests quietly
cargo test --quiet
```

### Running Examples
```bash
# Run modular ADSR melody example
cargo run --example modular_adsr_melody

# Run simple tone example
cargo run --example simple_tone
```

### Linting and Formatting
```bash
# Format code
cargo fmt

# Check formatting without applying
cargo fmt -- --check

# Run clippy linter
cargo clippy

# Run clippy with pedantic lints
cargo clippy -- -W clippy::pedantic

# Auto-fix clippy issues
cargo clippy --fix
```

## Architecture Overview

Fugue is a modular synthesis library for algorithmic music composition. It uses a signal-flow architecture inspired by Eurorack modular synthesizers.

### Codebase Structure

The codebase is organized by domain rather than technical concerns:

```
src/
├── lib.rs                    # Main library exports
├── traits.rs                 # Core Module trait
├── modules/                  # All synthesis modules
│   ├── clock/                # Clock and Tempo
│   ├── oscillator/           # Oscillator, OscillatorType
│   ├── lfo/                  # Lfo (low frequency oscillator for modulation)
│   ├── filter/               # Filter, FilterType (resonant filter)
│   ├── melody/               # MelodyGenerator, MelodyParams
│   ├── adsr/                 # Adsr envelope generator
│   ├── vca/                  # Vca (voltage-controlled amplifier)
│   └── dac/                  # Dac (audio output)
├── patch/                    # Declarative patch system
│   ├── format.rs             # JSON patch format (Patch, ModuleSpec, Connection)
│   ├── builder.rs            # PatchBuilder - constructs patches from JSON
│   ├── runtime.rs            # PatchRuntime, RunningPatch - manages execution
│   └── graph.rs              # SignalGraph - pull-based signal processing
└── music/                    # Music theory utilities
    ├── mod.rs                # Scale struct
    ├── note.rs               # Note struct
    └── mode.rs               # Mode enum
```

**Key naming conventions**:
- All "Modular" prefixes have been removed (e.g., `ModularAdsr` → `Adsr`, `ModularPatchBuilder` → `PatchBuilder`)
- Modules use directory-based organization with `mod.rs` as the main file
- Related types are co-located in the same directory

## IMPORTANT: Signal Routing Architecture

**Status**: ✅ **Complete** - The codebase uses a **pull-based signal processing architecture** with named port routing.

### Named Port Architecture

**Design principle**: Like real modular synthesizers, all signals are just voltages (f32 values). Modules interpret them based on which input port receives them.

**Key features**:
1. **Uniform signal type**: All signals are `f32` values
2. **Named ports**: Each module declares its inputs/outputs explicitly
   ```rust
   impl Module for Oscillator {
       fn inputs(&self) -> &[&str] { &["frequency", "fm", "am"] }
       fn outputs(&self) -> &[&str] { &["audio"] }
       fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> { ... }
       fn get_output(&self, port: &str) -> Result<f32, String> { ... }
       // ... plus caching methods
   }
   ```
3. **Explicit routing**: Connections specify port names in JSON
   ```json
   {
     "from": "clock", "from_port": "gate",
     "to": "adsr", "to_port": "gate"
   }
   ```

### Pull-Based Signal Processing

The system uses **pull-based processing** where the DAC recursively requests inputs from connected modules:

**How it works**:
1. DAC requests its inputs for the current sample
2. Each module recursively pulls from its dependencies (depth-first traversal)
3. Modules cache their outputs per sample to avoid reprocessing
4. Natural dependency resolution ensures correct processing order

**Key advantages**:
- **Correct ordering**: Recursive pull ensures modules process in dependency order
- **Efficient**: Each module processes exactly once per sample (via caching)
- **Simple**: No complex topological sorting or iterative scheduling
- **Deterministic**: Same results every time (no push-based race conditions)

**Architecture files**:
- `src/traits.rs` - Module trait with caching methods
- `src/patch/graph.rs` - Pull-based signal graph implementation

### Module Implementation Guide

To implement the `Module` trait for a new module:

```rust
use crate::Module;

pub struct MyModule {
    // Your module state
    input_value: f32,
    output_value: f32,
    
    // Required for pull-based caching
    last_processed_sample: u64,
}

impl Module for MyModule {
    fn name(&self) -> &str {
        "MyModule"
    }
    
    fn process(&mut self) -> bool {
        // Your DSP logic here
        self.output_value = self.input_value * 2.0;  // Example
        true
    }
    
    fn inputs(&self) -> &[&str] {
        &["input_port_name"]
    }
    
    fn outputs(&self) -> &[&str] {
        &["output_port_name"]
    }
    
    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "input_port_name" => {
                self.input_value = value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port))
        }
    }
    
    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "output_port_name" => Ok(self.output_value),
            _ => Err(format!("Unknown output port: {}", port))
        }
    }
    
    // Caching methods for pull-based processing
    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }
    
    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }
}
```

### Cycle Detection

The system **only supports acyclic graphs** (no feedback loops). Cycles are detected during patch validation using depth-first search.

**Why no cycles?**
- Pull-based processing would cause infinite recursion
- Future: Add delay modules for controlled feedback

**Error handling**:
- Validation fails with clear error message if cycle detected
- Example: `"Cycle detected in signal graph involving module 'osc1'"`

### Module Implementations

All modules implement the `Module` trait:

| Module | Location | Inputs | Outputs |
|--------|----------|--------|---------|
| `Clock` | `modules/clock/mod.rs` | _(none)_ | `gate` |
| `MelodyGenerator` | `modules/melody/mod.rs` | `gate` | `frequency`, `gate` |
| `Oscillator` | `modules/oscillator/mod.rs` | `frequency`, `fm`, `am` | `audio` |
| `Lfo` | `modules/lfo/mod.rs` | `sync`, `rate` | `out`, `out_uni` |
| `Filter` | `modules/filter/mod.rs` | `audio`, `cutoff`, `cutoff_cv`, `resonance` | `audio` |
| `Adsr` | `modules/adsr/mod.rs` | `gate`, `attack`, `decay`, `sustain`, `release` | `envelope` |
| `Vca` | `modules/vca/mod.rs` | `audio`, `cv` | `audio` |

## Signal Philosophy

**All signals are raw `f32` values** - like voltages in real modular synthesizers. Port names determine how modules interpret values:

- **`"audio"`** - Audio-rate waveforms
- **`"gate"`** - Trigger signals (1.0 = on, 0.0 = off)
- **`"frequency"`** - Pitch in Hz
- **`"envelope"`** - Amplitude envelope (0.0-1.0)
- **`"cv"`** - Control voltage for modulation
- **`"fm"`** - Frequency modulation input
- **`"am"`** - Amplitude modulation input

This uniform approach enables flexible routing: any output can connect to any compatible input.

### Core Modules

| Module | Location | Purpose | Key Ports |
|--------|----------|---------|-----------|
| `Clock` | `modules/clock/` | Tempo-driven timing | out: `gate` |
| `Tempo` | `modules/clock/tempo.rs` | Thread-safe BPM control | (shared state) |
| `Oscillator` | `modules/oscillator/` | Waveform generation | in: `frequency`, `fm`, `am`; out: `audio` |
| `Lfo` | `modules/lfo/` | Low-frequency modulation | in: `sync`, `rate`; out: `out`, `out_uni` |
| `Filter` | `modules/filter/` | Resonant filter (LP/HP/BP) | in: `audio`, `cutoff`, `cutoff_cv`, `resonance`; out: `audio` |
| `MelodyGenerator` | `modules/melody/` | Algorithmic note sequencing | in: `gate`; out: `frequency`, `gate` |
| `Adsr` | `modules/adsr/` | ADSR envelope generator | in: `gate`; out: `envelope` |
| `Vca` | `modules/vca/` | Voltage-controlled amplifier | in: `audio`, `cv`; out: `audio` |
| `Dac` | `modules/dac/` | Audio output via cpal | in: `audio` (from closure) |
| `Scale`/`Mode`/`Note` | `music/` | Music theory utilities | (data structures) |

## Declarative Patch System

Fugue supports both declarative (JSON) and programmatic (Rust) approaches for building synthesis patches.

### Declarative Approach (JSON)

Load and run a patch from JSON:
```rust
let patch = Patch::from_file("my_patch.json")?;
let dac = Dac::new()?;
let builder = PatchBuilder::new(dac.sample_rate());
let runtime = builder.build_and_run(patch)?;
let running = runtime.start()?;

// Control parameters at runtime
running.tempo().set_bpm(140.0);
running.melody_params().set_note_weights(vec![1.0, 0.5, 1.0]);
```

### Programmatic Approach

The declarative JSON approach is the primary API. The old programmatic API with `.connect()` chaining has been superseded by the module system.

### Supported Patch Modules

- **clock** - Timing and tempo
- **melody** - Algorithmic melody generation
- **voice** - Note-to-audio conversion with oscillator
- **oscillator** - Standalone oscillator for FM/AM synthesis
- **dac** - Audio output

### FM/AM Synthesis

Oscillators support named ports for modulation:
```json
{
  "connections": [
    {"from": "modulator", "to": "carrier", "to_port": "fm"},
    {"from": "carrier", "to": "dac"}
  ]
}
```

Supported ports:
- `"fm"` - Frequency modulation input
- `"am"` - Amplitude modulation input

### Multiple Voices / Parallel Paths

The system supports multiple parallel signal paths that automatically mix at the DAC:
```json
{
  "connections": [
    {"from": "clock", "to": "melody1"},
    {"from": "clock", "to": "melody2"},
    {"from": "melody1", "to": "voice1"},
    {"from": "melody2", "to": "voice2"},
    {"from": "voice1", "to": "dac"},
    {"from": "voice2", "to": "dac"}
  ]
}
```

## Code Style Guidelines

### Imports
- Use explicit imports, avoid glob imports except for preludes
- Group imports: std library, external crates, then local crate modules
- Example:
  ```rust
  use crate::Module;
  use std::f32::consts::PI;
  ```

### Formatting
- Use 4-space indentation (Rust standard)
- Max line length: typically 100 characters (Rust convention)
- Use trailing commas in multi-line lists/structs
- Place opening braces on same line as declaration

### Types and Traits
- Use explicit type annotations for public APIs
- Prefer `f32` for audio/DSP (performance), `f64` for timing (precision)
- Use `u32` for sample rates, `u64` for sample counts
- Implement `Clone`, `Copy`, and `Debug` where appropriate
- Use builder pattern for optional parameters (`.with_*()` methods)

### Naming Conventions
- Types: `PascalCase` (e.g., `OscillatorType`, `ClockSignal`)
- Functions/methods: `snake_case` (e.g., `process_signal`, `next_sample`)
- Constants: `SCREAMING_SNAKE_CASE`
- Type parameters: single capital letter or `PascalCase` (e.g., `T`, `TIn`, `TOut`)

### Thread Safety Pattern
- Use `Arc<Mutex<T>>` for shared state between threads
- Wrap common patterns in `Control<T>` type
- Main thread sets values, audio thread reads
- Example:
  ```rust
  let tempo = Tempo::new(120.0);  // Returns Arc<Mutex<f64>> internally
  tempo.set_bpm(140.0);  // Main thread
  let bpm = tempo.get_bpm();  // Audio thread reads
  ```

### Error Handling
- Use `Result<T, Box<dyn std::error::Error>>` for main functions
- Use `.unwrap()` for `Mutex::lock()` (poisoning is rare in audio contexts)
- Clamp values to valid ranges using `.clamp()` (e.g., resonance 0.0-1.0)
- Validate inputs in constructors

### Documentation
- Add doc comments for public types and functions
- Explain the metaphorical/musical meaning (e.g., "Like a master clock in Eurorack")
- Document parameter ranges and units (Hz, beats, 0.0-1.0, etc.)
- Include usage examples for complex APIs

### Module Implementation Pattern
```rust
impl Module for MyModule {
    fn name(&self) -> &str {
        "MyModule"
    }
    
    fn process(&mut self) -> bool {
        // Per-sample DSP processing
        true
    }
    
    fn inputs(&self) -> &[&str] {
        &["input_port"]
    }
    
    fn outputs(&self) -> &[&str] {
        &["output_port"]
    }
    
    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "input_port" => { /* set value */ Ok(()) }
            _ => Err(format!("Unknown port: {}", port))
        }
    }
    
    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "output_port" => Ok(self.output_value),
            _ => Err(format!("Unknown port: {}", port))
        }
    }
    
    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }
    
    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }
}
```

### Best Practices
- Keep audio-thread code allocation-free (no `Vec::new()`, etc.)
- Use pre-allocated buffers for DSP
- Prefer `f32` math for audio-rate signals (SIMD-friendly)
- Reset phase accumulators using `%=` to prevent drift
- Scale audio output to prevent clipping (e.g., `* 0.3`)
- Use `Send` marker for thread-safe types

## Music Theory Reference

### Modes
All 7 diatonic modes are supported:
- **Ionian** (Major) - Happy, bright
- **Dorian** - Jazzy, balanced minor
- **Phrygian** - Spanish, exotic minor
- **Lydian** - Dreamy, floating major
- **Mixolydian** - Bluesy, dominant major
- **Aeolian** (Natural Minor) - Sad, dark
- **Locrian** - Unstable, tense

### MIDI Notes
- Middle C = MIDI note 60 = 261.63 Hz
- Concert A = MIDI note 69 = 440 Hz
- Use `Note::new(midi_number)` or `Note::from_frequency(hz)`

### Oscillator Types
- **Sine** - Pure, smooth, no overtones
- **Square** - Hollow, retro, odd harmonics
- **Sawtooth** - Bright, full harmonics
- **Triangle** - Mellow, soft, odd harmonics
