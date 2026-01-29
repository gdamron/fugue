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
├── traits.rs                 # Core traits: Module, Generator, Processor, ModularModule
├── signal.rs                 # Signal types: Audio, ClockSignal, FrequencySignal, NoteSignal
├── modules/                  # All synthesis modules
│   ├── clock/                # Clock and Tempo
│   ├── oscillator/           # Oscillator, ModulatedOscillator, OscillatorType
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
   impl ModularModule for Oscillator {
       fn inputs(&self) -> &[&str] { &["frequency", "fm", "am"] }
       fn outputs(&self) -> &[&str] { &["audio"] }
       fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> { ... }
       fn get_output(&self, port: &str) -> Result<f32, String> { ... }
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
- `src/traits.rs` - ModularModule trait with caching methods
- `src/patch/graph.rs` - Pull-based signal graph implementation

### Module Implementation Guide

To implement the `ModularModule` trait for a new module:

```rust
use crate::traits::{Module, ModularModule};

pub struct MyModule {
    // Your module state
    input_value: f32,
    output_value: f32,
    
    // Required for pull-based caching
    last_processed_sample: u64,
}

impl ModularModule for MyModule {
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
    
    fn reset_inputs(&mut self) {
        // Reset control signals (gates, triggers) to default
        // Don't reset configuration parameters
        self.input_value = 0.0;  // Example: gate resets to 0
    }
    
    // Caching methods for pull-based processing
    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }
    
    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }
    
    fn get_cached_output(&self, port: &str) -> Result<f32, String> {
        // Usually just calls get_output()
        self.get_output(port)
    }
}

impl Module for MyModule {
    fn process(&mut self) -> bool {
        // Your DSP logic here
        self.output_value = self.input_value * 2.0;  // Example
        true
    }
    
    fn name(&self) -> &str {
        "MyModule"
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

All modules implement the `ModularModule` trait:

| Module | Location | Inputs | Outputs |
|--------|----------|--------|---------|
| `Clock` | `modules/clock/mod.rs` | _(none)_ | `gate` |
| `MelodyGenerator` | `modules/melody/mod.rs` | `gate` | `frequency`, `gate` |
| `Oscillator` | `modules/oscillator/mod.rs` | `frequency`, `fm`, `am` | `audio` |
| `Adsr` | `modules/adsr/mod.rs` | `gate`, `attack`, `decay`, `sustain`, `release` | `envelope` |
| `Vca` | `modules/vca/mod.rs` | `audio`, `cv` | `audio` |

### Migration Strategy

**Current state**: Both old type-based and new modular systems coexist.
- Old system: `Generator<T>` and `Processor<TIn, TOut>` (legacy)
- New system: `ModularModule` with pull-based processing (recommended)

**Don't break existing code!** Old examples still work.

## Signal Types (`src/signal.rs`)

Two fundamental signal categories:

1. **`Audio`** - Audio-rate signals (44.1kHz)
   - Carries waveforms, CV, gates, triggers, envelopes
   - Single `f32` value per sample
   - Like voltage flowing through Eurorack patch cables

2. **`Control<T>`** - Thread-safe parameters
   - User input (knob positions, button states, etc.)
   - Wrapped in `Arc<Mutex<T>>` for concurrent access
   - Can be updated from UI thread while audio thread reads

Compound signal types:
- `ClockSignal` - Timing info (beats, phase, measure)
- `FrequencySignal` - Pitch in Hz
- `NoteSignal` - Gate + frequency for musical notes

### Module System (`src/module.rs`)

All components implement traits from `module.rs`:

- **`Module`** - Base trait with `process()` for per-sample advancement
- **`Generator<T>`** - Creates signals without input (Clock, Oscillator, Sequencer)
- **`Processor<TIn, TOut>`** - Transforms signals (Filter, Voice, effects)

Modules connect using `.connect()` for signal chaining:
```rust
let audio_gen = clock.connect(sequencer).connect(voice);
```

### Core Modules

| Module | Location | Purpose |
|--------|----------|---------|
| `Clock` | `modules/clock/` | Tempo-driven timing, outputs `ClockSignal` |
| `Tempo` | `modules/clock/tempo.rs` | Thread-safe BPM control |
| `Oscillator` | `modules/oscillator/` | Waveform generation (sine, square, saw, triangle) |
| `MelodyGenerator` | `modules/melody/` | Probabilistic note selection from scale |
| `Adsr` | `modules/adsr/` | ADSR envelope generator |
| `Vca` | `modules/vca/` | Voltage-controlled amplifier |
| `Dac` | `modules/dac/` | Audio output via cpal |
| `Scale`/`Mode`/`Note` | `music/` | Music theory (modes, MIDI↔frequency) |

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

Build modules and connect them in Rust code:
```rust
let clock = Clock::new(sample_rate, tempo.clone());
let melody = MelodyGenerator::new(scale, params, sample_rate, tempo);
let voice = Voice::new(sample_rate, OscillatorType::Sine);
let audio_gen = clock.connect(melody).connect(voice);
dac.start(audio_gen)?;
```

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
- Example from `oscillator.rs`:
  ```rust
  use crate::{
      module::{Generator, Module, Processor},
      AudioSignal, FrequencySignal,
  };
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
// 1. Implement Module trait
impl Module for MyModule {
    fn process(&mut self) -> bool {
        // Per-sample processing
        true  // Return false to remove module from chain
    }
    
    fn name(&self) -> &str {
        "MyModule"
    }
}

// 2. Implement Generator OR Processor
impl Generator<OutputType> for MyModule {
    fn output(&mut self) -> OutputType {
        // Generate signal
    }
}
// OR
impl Processor<InputType, OutputType> for MyModule {
    fn process_signal(&mut self, input: InputType) -> OutputType {
        // Transform signal
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
