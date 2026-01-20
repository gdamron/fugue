# Fugue

A Rust library for composing algorithmic and generative music, inspired by ChucK and Eurorack modular synthesis.

## Features

- 🎵 **Cross-platform**: Runs on desktop via CLI, designed for future WebAssembly support
- ⏰ **Precise time control**: Built-in clock and tempo management for musical timing
- 🔊 **Audio synthesis**: Multiple oscillator types (sine, square, sawtooth, triangle)
- 🎹 **Music theory**: Scale and mode support (Dorian, Ionian, Phrygian, etc.)
- 🎲 **Algorithmic composition**: Probabilistic melody generation with live parameter updates
- 🎚️ **Live control**: Update scales, rhythms, and synthesis parameters in real-time
- 📄 **Declarative patches**: Define synthesis setups using JSON documents

## Quick Start

### Declarative Approach (Recommended)

Run a patch defined in JSON:

```bash
cargo run --example dorian_melody_declarative
```

This loads `examples/dorian_melody.json` and runs the patch. You can control it with:

- `1-7`: Toggle individual scale degrees on/off
- `s/w/t/q`: Switch oscillators (Sine/Sawtooth/Triangle/Square)
- `+/-`: Adjust tempo
- `f/n`: Make notes faster or slower
- `r`: Emphasize root and fifth notes
- `x`: Exit

### Programmatic Approach

Run the imperative example:

```bash
cargo run --example dorian_melody
```

## Two Ways to Build

Fugue supports both declarative and programmatic approaches to building synthesis patches.

### Declarative (JSON Patches)

Define your patch in a JSON file:

```json
{
  "version": "1.0.0",
  "title": "My Patch",
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

let patch = Patch::from_file("my_patch.json")?;
let dac = Dac::new()?;
let builder = PatchBuilder::new(dac.sample_rate());
let runtime = builder.build_and_run(patch)?;
let running = runtime.start()?;

// Control parameters at runtime
running.tempo().set_bpm(140.0);
running.melody_params().set_note_duration(0.5);
```

See [DECLARATIVE.md](DECLARATIVE.md) for full documentation of the patch format.

### Programmatic (Rust Code)

Build patches imperatively in code:

```rust
use fugue::*;

let mut dac = Dac::new()?;
let sample_rate = dac.sample_rate();
let tempo = Tempo::new(120.0);

// Create modules
let clock = Clock::new(sample_rate, tempo.clone());
let scale = Scale::new(Note::new(60), Mode::Dorian);
let params = MelodyParams::new(vec![0, 1, 2, 3, 4, 5, 6]);
let melody = MelodyGenerator::new(scale, params.clone(), sample_rate, tempo.clone());
let voice = Voice::new(sample_rate, OscillatorType::Sine);

// Connect the chain
let audio_gen = clock.connect(melody).connect(voice);

// Start audio
dac.start(audio_gen)?;
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

- [x] Declarative patch system with JSON format
- [ ] Additional synthesis: FM synthesis, noise generators, envelopes
- [ ] Effects: Reverb, delay, distortion
- [ ] MIDI support: Input and output
- [ ] Pattern sequencing: Multi-track composition
- [ ] WebAssembly: Browser support
- [ ] Visualization: Waveform and spectrum display
- [ ] Saving/loading: Export audio and save compositions
- [ ] Business logic injection in patches (custom code hooks)
- [ ] Real-time control mapping and automation

## License

MIT

## Contributing

Contributions welcome! Please open an issue or PR.
