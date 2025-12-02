# Fugue Project Summary

## Overview
Fugue is a Rust library for algorithmic and generative music composition, inspired by ChucK and Eurorack modular synthesis.

## ✅ Completed Features

### Core Architecture
- **Time Management**: Precise clock and tempo system with BPM control
- **Audio Synthesis**: Multiple oscillator types (sine, square, sawtooth, triangle)
- **Music Theory**: Full scale and mode support (all 7 diatonic modes)
- **Algorithmic Composition**: Probabilistic melody generation with weighted note selection
- **Real-time Control**: All parameters updateable while audio is playing
- **Cross-platform Audio**: cpal-based audio engine for desktop platforms

### Project Structure
```
fugue/
├── src/
│   ├── lib.rs          # Main library exports
│   ├── time.rs         # Clock and tempo management
│   ├── synthesis.rs    # Oscillators and filters
│   ├── scale.rs        # Musical scales and modes
│   ├── sequencer.rs    # Melody generation
│   └── audio.rs        # Audio engine
├── examples/
│   └── dorian_melody.rs # Interactive example
├── README.md
├── GETTING_STARTED.md
├── MUSIC_REFERENCE.md
└── Cargo.toml
```

## Example Usage

```rust
use fugue::*;

// Setup
let tempo = Tempo::new(120.0);
let scale = Scale::new(Note::new(62), Mode::Dorian);
let params = MelodyParams::new(vec![0, 1, 2, 3, 4, 5, 6]);
let melody_gen = MelodyGenerator::new(scale, params.clone());

// Start audio
let mut engine = AudioEngine::new()?;
engine.start_melody(melody_gen, tempo.clone())?;

// Live updates
params.set_oscillator_type(OscillatorType::Sawtooth);
tempo.set_bpm(140.0);
```

## Interactive Demo

Run with: `cargo run --example dorian_melody`

The example provides:
- Real-time scale degree toggling
- Oscillator type switching
- Tempo adjustment
- Note duration control
- Weighted probability presets

## Key Design Decisions

1. **Arc<Mutex<>> for Shared State**: Enables live parameter updates from any thread
2. **StdRng for Audio Thread**: Uses SeedableRng for Send compatibility
3. **Simple ASR Envelope**: 10% attack, 80% sustain, 10% release
4. **Generic Sample Format**: Supports F32, I16, and U16 via cpal
5. **Modular Architecture**: Each component can be used independently

## Technical Highlights

- **Thread-safe parameter updates**: All parameters use Arc<Mutex<>> for safe concurrent access
- **Sample-accurate timing**: Clock tracks individual samples for precise musical timing
- **Weighted probability**: Flexible melody generation with customizable note weights
- **Multiple waveforms**: Four classic oscillator types with smooth switching
- **Mode flexibility**: All seven diatonic modes supported

## Documentation

- `README.md`: Project overview and quick start
- `GETTING_STARTED.md`: Detailed guide with examples
- `MUSIC_REFERENCE.md`: Musical concepts and theory reference

## Future Enhancements (Roadmap)

### Near Term
- [ ] ADSR envelopes (attack, decay, sustain, release)
- [ ] LFO (Low Frequency Oscillator) for modulation
- [ ] Multiple simultaneous voices
- [ ] Rhythm patterns and sequencing

### Medium Term
- [ ] FM synthesis
- [ ] Audio effects (reverb, delay, filter)
- [ ] MIDI input/output
- [ ] Save/load compositions

### Long Term
- [ ] WebAssembly support for browser
- [ ] Visual waveform/spectrum display
- [ ] Plugin system for custom generators
- [ ] Advanced synthesis (granular, wavetable)

## Dependencies

- `cpal` (0.15): Cross-platform audio I/O
- `rand` (0.8): Random number generation
- `wasm-bindgen` (0.2): WebAssembly bindings (prepared for future use)

## Performance

- Builds cleanly with zero errors
- Minimal warnings (suppressed false positives)
- Optimized release builds (~7.5s build time)
- Low latency audio with cpal defaults

## Design Philosophy

**Strongly-timed**: Time is a first-class concept, following ChucK's model
**Live-codeable**: All parameters can change while audio is running
**Modular**: Components compose naturally
**Simple first**: Start with basics, build complexity through composition

## WebAssembly Readiness

The project is structured for future WASM support:
- Dependencies include wasm-bindgen
- Audio abstraction layer (can swap cpal for Web Audio API)
- No platform-specific code in core modules
- Minimal external dependencies

## License

MIT

---

**Status**: ✅ Fully functional - ready for algorithmic composition!
