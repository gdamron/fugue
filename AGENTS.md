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
# Run main example (Dorian melody with live controls)
cargo run --example dorian_melody

# Run other examples
cargo run --example modular_clock
cargo run --example modular_chain
cargo run --example modular_voice
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

## IMPORTANT: Signal Routing Architecture (Current Redesign)

**Status**: The codebase is undergoing a fundamental architectural change to enable flexible signal routing.

### The Problem

The original architecture used **type-based signal routing** via Rust generics:
- `Generator<T>` and `Processor<TIn, TOut>` enforce signal compatibility at compile time
- Connections between modules are implicit based on their types
- This prevents flexible routing patterns common in modular synthesis

**Example of what doesn't work**:
- Routing a clock trigger to an ADSR gate input (type mismatch: `ClockSignal` vs `NoteSignal`)
- Using an envelope to control a VCA (no way to route envelope output to amplitude control)
- Arbitrary CV routing (e.g., LFO modulating filter cutoff)

The issue was discovered when trying to implement proper ADSR envelope control. Sequencers output brief triggers, but envelopes need sustained gates. The type system prevented routing signals between these modules.

### The Solution: Named Port Architecture

**Design principle**: Like real modular synthesizers, all signals are just voltages (f32 values). Modules interpret them based on which input port receives them.

**Key changes**:
1. **Uniform signal type**: All signals become `f32` (or `Signal(f32)` wrapper)
2. **Named ports**: Each module declares its inputs/outputs explicitly
   ```rust
   impl ModularModule for Oscillator {
       fn inputs(&self) -> Vec<&str> { vec!["frequency", "fm", "am"] }
       fn outputs(&self) -> Vec<&str> { vec!["audio"] }
       fn set_input(&mut self, port: &str, value: f32) { ... }
       fn get_output(&mut self, port: &str) -> f32 { ... }
   }
   ```
3. **Explicit routing**: Connections must specify port names
   ```json
   {
     "from": "clock", "from_port": "trigger",
     "to": "adsr", "to_port": "gate"
   }
   ```

### Implementation Status

**Completed**:
- ✅ `ModularModule` trait created (`src/module/modular.rs`)
- ✅ VCA module with named ports (`src/synthesis/vca.rs`)
  - Inputs: `audio`, `cv`
  - Outputs: `audio`
- ✅ ModularAdsr module with named ports (`src/synthesis/modular_adsr.rs`)
  - Inputs: `gate`, `attack`, `decay`, `sustain`, `release`
  - Outputs: `envelope`
- ✅ Clock module implements `ModularModule` (`src/time/clock.rs`)
  - Inputs: none (it's a source)
  - Outputs: `trigger`, `beat`, `phase`, `measure`
- ✅ Oscillator implements `ModularModule` (`src/oscillator/mod.rs`)
  - Inputs: `frequency`, `fm`, `am`
  - Outputs: `audio`
- ✅ MelodyGenerator implements `ModularModule` (`src/sequencer/melody_generator.rs`)
  - Inputs: `beat`, `phase`
  - Outputs: `frequency`, `gate`, `trigger`

**Remaining work**:
1. Update `PatchBuilder` (`src/builder.rs`) to route using port names when `from_port`/`to_port` are specified
2. Update `Dac` to accept modular inputs (currently expects `Generator<Audio>`)
3. Create example patch demonstrating ADSR envelope control

**Migration plan**:
- Old type-based system still works and should not be broken
- New modular system coexists alongside old system
- PatchBuilder should detect which system to use based on presence of port names in connections

### Migration Strategy

**Don't break existing code!** The type-based system still works. New modules should use the named port system. Both can coexist during migration.

### Signal Types (`src/signal.rs`)

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
| `Clock` | `time.rs` | Tempo-driven timing, outputs `ClockSignal` |
| `Tempo` | `time.rs` | Thread-safe BPM control |
| `Oscillator` | `oscillator.rs` | Waveform generation (sine, square, saw, triangle) |
| `Voice` | `synthesis.rs` | Converts `NoteSignal` to audio with envelope |
| `MelodyGenerator` | `sequencer.rs` | Probabilistic note selection from scale |
| `Filter` | `synthesis.rs` | Low-pass filter for audio processing |
| `Scale`/`Mode`/`Note` | `scale.rs` | Music theory (modes, MIDI↔frequency) |
| `Dac` | `modular_audio.rs` | Audio output via cpal |

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
running.melody_params().set_note_duration(0.5);
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
