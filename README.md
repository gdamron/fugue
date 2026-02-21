# Fugue

A Rust library for composing algorithmic and generative music, inspired by ChucK and Eurorack modular synthesis.

## Features

- 🎵 **Cross-platform**: Runs on desktop via CLI, designed for future WebAssembly support
- ⏰ **Precise time control**: Built-in clock and tempo management for musical timing
- 🔊 **Audio synthesis**: Multiple oscillator types (sine, square, sawtooth, triangle)
- 🎹 **Music theory**: Scale and mode support (Dorian, Ionian, Phrygian, etc.)
- 🎲 **Algorithmic composition**: Probabilistic melody generation with live parameter updates
- 🎚️ **Live control**: Update scales, rhythms, and synthesis parameters in real-time
- 📄 **Inventions**: Define synthesis setups using JSON documents

## Quick Start

### Run the Examples

Fugue includes three examples demonstrating the modular routing system:

**1. Simple Tone** - Minimal working example
```bash
cargo run --example simple_tone
```
Demonstrates: Clock (PWM gate) → ADSR → VCA + Oscillator(440Hz) → DAC

**2. ADSR Melody** - Clean melody with envelope shaping
```bash
cargo run --example modular_adsr_melody
```
Demonstrates: Clock → MelodyGenerator → Oscillator → VCA with ADSR envelope control

**3. Interactive Dorian Melody** - Real-time control
```bash
cargo run --example dorian_melody_declarative
```
Generates an infinite melody in D Dorian mode with live controls:
- `s/w/t/q` - Switch waveforms (Sine/Sawtooth/Triangle/Square)
- `1-7` - Toggle scale degrees on/off
- `+/-` - Adjust tempo by 10 BPM
- `f/n` - Change note duration (faster/slower)
- `r` - Emphasize root and fifth notes
- `x` - Exit

### Your First Program

Create `examples/my_melody.rs`:

```rust
use fugue::*;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a tempo
    let tempo = Tempo::new(140.0);
    
    // Create a scale (C Major)
    let root = Note::new(60);  // Middle C
    let scale = Scale::new(root, Mode::Ionian);
    
    // Set up which scale degrees to use
    let allowed_degrees = vec![0, 2, 4];  // I, III, V (major triad)
    let params = MelodyParams::new(allowed_degrees);
    
    // Create the melody generator
    let melody_gen = MelodyGenerator::new(scale, params.clone());
    
    // Start audio
    let mut engine = AudioEngine::new()?;
    engine.start_melody(melody_gen, tempo)?;
    
    println!("Playing C Major triad melody...");
    thread::sleep(Duration::from_secs(10));
    
    Ok(())
}
```

Run it:
```bash
cargo run --example my_melody --release
```

## Two Ways to Build

Fugue supports both declarative and programmatic approaches to building synthesis setups.

### Declarative (JSON)

Define your invention in a JSON file:

```json
{
  "version": "1.0.0",
  "title": "My Invention",
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

let invention = Invention::from_file("my_invention.json")?;
let dac = Dac::new()?;
let builder = InventionBuilder::new(dac.sample_rate());
let runtime = builder.build_and_run(invention)?;
let running = runtime.start()?;

// Control parameters at runtime
running.tempo().set_bpm(140.0);
running.melody_params().set_note_weights(vec![1.0, 0.5, 1.0]);
```

See [DECLARATIVE.md](DECLARATIVE.md) for full documentation of the invention format.

### Programmatic (Rust Code)

Build setups imperatively in code:

```rust
use fugue::*;

let mut dac = Dac::new()?;
let sample_rate = dac.sample_rate();
let tempo = Tempo::new(120.0);

// Create modules
let clock = Clock::new(sample_rate, tempo.clone());
let scale = Scale::new(Note::new(60), Mode::Dorian);
let params = MelodyParams::new(vec![0, 1, 2, 3, 4, 5, 6]);
let melody = MelodyGenerator::new(scale, params.clone());
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

## Musical Modes Reference

Fugue supports all seven diatonic modes:

- **Ionian** (Major) - Happy, bright
- **Dorian** - Jazzy, balanced minor
- **Phrygian** - Spanish, exotic minor
- **Lydian** - Dreamy, floating major
- **Mixolydian** - Bluesy, dominant major
- **Aeolian** (Natural Minor) - Sad, dark
- **Locrian** - Unstable, tense

Example:
```rust
let scale = Scale::new(Note::new(60), Mode::Dorian);
```

## MIDI Notes Reference

Common MIDI note numbers:

- C4 (Middle C) = 60 = 261.63 Hz
- D4 = 62 = 293.66 Hz
- A4 (Concert pitch) = 69 = 440.00 Hz

```rust
let middle_c = Note::new(60);
let freq = middle_c.frequency();  // 261.63 Hz
```

## WebAssembly Support (Coming Soon)

The library is designed for WebAssembly support. Future versions will include:

- Browser-based audio via Web Audio API
- Interactive web interfaces for live control
- MIDI input/output support

## Roadmap

- [x] Declarative invention system with JSON format
- [ ] Additional synthesis: FM synthesis, noise generators, envelopes
- [ ] Effects: Reverb, delay, distortion
- [ ] MIDI support: Input and output
- [ ] Pattern sequencing: Multi-track composition
- [ ] WebAssembly: Browser support
- [ ] Visualization: Waveform and spectrum display
- [ ] Saving/loading: Export audio and save compositions
- [ ] Business logic injection in invention files (custom code hooks)
- [ ] Real-time control mapping and automation

## License

MIT

## Contributing

Contributions welcome! Please open an issue or PR.
