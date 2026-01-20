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
