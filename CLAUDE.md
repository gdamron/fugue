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

## CRITICAL: Ongoing Architectural Redesign

**DO NOT use the old type-based routing system for new modules!** The codebase is transitioning to a named port architecture.

### Why the Change?

The original type-based system (`Generator<T>`, `Processor<TIn, TOut>`) prevents flexible signal routing:
- Clock triggers can't trigger ADSR envelopes (type mismatch)
- Envelopes can't control VCA amplitude (no routing path)
- Can't do arbitrary CV modulation (LFO → filter cutoff, etc.)

**The insight**: In real modular synths, all signals are just voltages. Modules interpret them based on which INPUT PORT receives them, not based on the signal's "type".

### New Architecture: Named Ports

All signals are `f32` values. Modules declare their ports explicitly:

```rust
// Old way (DON'T DO THIS for new modules)
impl Processor<NoteSignal, Audio> for Voice { ... }

// New way (DO THIS)
impl ModularModule for VCA {
    fn inputs(&self) -> &[&str] { &["audio", "cv"] }
    fn outputs(&self) -> &[&str] { &["audio"] }
    fn set_input(&mut self, port: &str, value: f32) { ... }
    fn get_output(&mut self, port: &str) -> f32 { ... }
}
```

Patches must specify port names:
```json
{
  "connections": [
    {"from": "adsr", "from_port": "envelope", "to": "vca", "to_port": "cv"},
    {"from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio"}
  ]
}
```

### Migration Status

- `Connection` struct already has `from_port`/`to_port` fields (currently optional)
- Old type-based system still works and should not be broken
- New `ModularModule` trait will coexist with old traits during migration
- Modules being added: ADSR, VCA (envelope control use case that revealed the issue)

### If Adding New Modules

1. Use the new `ModularModule` trait (see `src/module/modular.rs` if it exists)
2. All inputs/outputs are `f32` values
3. Declare explicit port names
4. Don't worry about type compatibility - that's the point!

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

## Declarative Patch System

Fugue supports both declarative (JSON) and programmatic (Rust) approaches.

### Load and Run a JSON Patch

```rust
let patch = Patch::from_file("my_patch.json")?;
let dac = Dac::new()?;
let builder = PatchBuilder::new(dac.sample_rate());
let runtime = builder.build_and_run(patch)?;
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
    {"from": "clock", "to": "melody1"},
    {"from": "clock", "to": "melody2"},
    {"from": "melody1", "to": "voice1"},
    {"from": "melody2", "to": "voice2"},
    {"from": "voice1", "to": "dac"},
    {"from": "voice2", "to": "dac"}
  ]
}
```
