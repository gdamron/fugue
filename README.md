# Fugue

A Rust library for composing algorithmic and generative music, inspired by ChucK and Eurorack modular synthesis.

## Features

- 🎵 **Cross-platform**: Runs on desktop via CLI, designed for future WebAssembly support
- ⏰ **Precise time control**: Built-in clock and tempo management for musical timing
- 🔊 **Audio synthesis**: Multiple oscillator types (sine, square, sawtooth, triangle)
- 🎹 **Music theory**: Scale and mode support (Dorian, Ionian, Phrygian, etc.)
- 🎲 **Algorithmic composition**: Probabilistic melody generation with live parameter updates
- 🎚️ **Live control**: Update scales, rhythms, and synthesis parameters in real-time

## Quick Start

Run the Dorian melody example:

```bash
cargo run --example dorian_melody
```

This will start playing a randomly generated melody in the Dorian mode. You can control it with:

- `1-7`: Toggle individual scale degrees on/off
- `s/w/t/q`: Switch oscillators (Sine/Sawtooth/Triangle/Square)
- `+/-`: Adjust tempo
- `f/n`: Make notes faster or slower
- `r`: Emphasize root and fifth notes
- `x`: Exit

## Architecture

### Core Modules

- **`time`**: Clock and tempo management for precise musical timing
- **`synthesis`**: Oscillators and filters for audio generation
- **`scale`**: Musical scales and modes with frequency calculation
- **`sequencer`**: Algorithmic melody generation with weighted probabilities
- **`audio`**: Cross-platform audio output via cpal

## Example Usage

```rust
use fugue::*;

// Create a tempo at 120 BPM
let tempo = Tempo::new(120.0);

// Create a D Dorian scale
let root = Note::new(62);  // D4
let scale = Scale::new(root, Mode::Dorian);

// Configure melody parameters
let allowed_degrees = vec![0, 1, 2, 3, 4, 5, 6];
let params = MelodyParams::new(allowed_degrees);

// Create melody generator
let melody_gen = MelodyGenerator::new(scale, params.clone());

// Start audio engine
let mut engine = AudioEngine::new()?;
engine.start_melody(melody_gen, tempo.clone())?;

// Update parameters live
params.set_oscillator_type(OscillatorType::Sawtooth);
params.set_note_duration(0.5);  // Half notes
tempo.set_bpm(140.0);
```

## Design Philosophy

Fugue draws inspiration from:

- **ChucK**: Strongly-timed programming model for music
- **Eurorack**: Modular approach to synthesis with voltage control
- **VCV Rack**: Virtual modular synthesis environment

The library emphasizes:

1. **Precise timing**: Musical time is a first-class concept
2. **Live coding**: Parameters can be updated while audio is running
3. **Modularity**: Components can be composed and reconfigured
4. **Simplicity**: Start with basic building blocks, compose complexity

## WebAssembly Support (Coming Soon)

The library is designed for WebAssembly support. Future versions will include:

- Browser-based audio via Web Audio API
- Interactive web interfaces for live control
- MIDI input/output support

## Roadmap

- [ ] Additional synthesis: FM synthesis, noise generators, envelopes
- [ ] Effects: Reverb, delay, distortion
- [ ] MIDI support: Input and output
- [ ] Pattern sequencing: Multi-track composition
- [ ] WebAssembly: Browser support
- [ ] Visualization: Waveform and spectrum display
- [ ] Saving/loading: Export audio and save compositions

## License

MIT

## Contributing

Contributions welcome! Please open an issue or PR.
