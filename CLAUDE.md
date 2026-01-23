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

# Run tests
cargo test

# Check for compile errors without building
cargo check
```

## Architecture Overview

Fugue is a Rust library for algorithmic/generative music composition using a modular synthesis approach inspired by Eurorack, ChucK, and WebAudio.

### Signal Types

Two fundamental signal types (see `src/signal.rs`):

- **`Audio`** - Audio-rate signals (44.1kHz). Carries sound waveforms, CV, gates, triggers, envelopes. Like voltage flowing through Eurorack patch cables.
- **`Control<T>`** - Thread-safe parameters (`Arc<Mutex<T>>`). User input like knob positions, button states, oscillator selection. Can be updated from UI thread while audio thread reads.

Compound signal types:
- `ClockSignal` - Timing info (beats, phase, measure)
- `FrequencySignal` - Pitch in Hz
- `NoteSignal` - Gate + frequency for musical notes

### Module System

All components implement traits from `src/module.rs`:

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
| `MelodyParams` | `sequencer.rs` | Thread-safe melody parameters |
| `Scale`/`Mode`/`Note` | `scale.rs` | Music theory (modes, MIDI↔frequency) |
| `Dac` | `modular_audio.rs` | Audio output via cpal |

### Typical Signal Flow

```
Clock (ClockSignal) → MelodyGenerator (NoteSignal) → Voice (Audio) → Dac
```

### Thread Safety Pattern

Shared state uses `Arc<Mutex<T>>` for lock-free-ish updates between main/audio threads. The `Control<T>` type wraps this pattern. Example:
```rust
params.set_oscillator_type(OscillatorType::Sawtooth);  // Main thread
osc_type.get()  // Audio thread reads latest value
```
