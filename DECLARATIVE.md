# Fugue - Declarative Patch System

This document describes the declarative patch system for Fugue, which allows you to define modular synthesis patches using JSON documents.

## Overview

The declarative patch system provides an alternative to the imperative/programmatic approach for building audio synthesis chains. Instead of writing Rust code to instantiate and connect modules, you can define the entire patch in a JSON document.

## Patch Document Format

A patch document is a JSON file with the following structure:

```json
{
  "version": "1.0.0",
  "title": "Patch Name",
  "description": "Optional description",
  "modules": [
    {
      "id": "unique_id",
      "type": "module_type",
      "config": {
        // module-specific configuration
      }
    }
  ],
  "connections": [
    {
      "from": "source_module_id",
      "to": "destination_module_id"
    }
  ]
}
```

### Fields

- **version**: Patch format version (currently "1.0.0")
- **title**: Optional human-readable name for the patch
- **description**: Optional longer description
- **modules**: Array of module specifications
- **connections**: Array of connections between modules (defines signal flow)

## Module Types

### Clock

Provides timing and tempo information.

```json
{
  "id": "clock",
  "type": "clock",
  "config": {
    "bpm": 120.0,
    "time_signature": {
      "beats_per_measure": 4,
      "beat_unit": 4
    }
  }
}
```

**Config Options:**
- `bpm` (optional, default: 120.0): Beats per minute
- `time_signature` (optional): Time signature configuration
  - `beats_per_measure`: Number of beats per measure
  - `beat_unit`: Note value that gets one beat

### Melody

Generates melodic sequences using scale degrees and probabilistic selection.

```json
{
  "id": "melody",
  "type": "melody",
  "config": {
    "root_note": 60,
    "mode": "dorian",
    "scale_degrees": [0, 1, 2, 3, 4, 5, 6],
    "note_weights": [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
    "note_duration": 1.0,
    "oscillator_type": "sine"
  }
}
```

**Config Options:**
- `root_note` (optional, default: 60): MIDI note number for the scale root
- `mode` (optional, default: "dorian"): Musical mode (ionian/major, dorian, phrygian, lydian, mixolydian, aeolian/minor, locrian)
- `scale_degrees` (optional, default: [0,1,2,3,4,5,6]): Which scale degrees to use
- `note_weights` (optional): Probability weights for each allowed degree
- `note_duration` (optional, default: 1.0): Duration in beats
- `oscillator_type` (optional, default: "sine"): Waveform type

### Voice

Converts note signals to audio using an oscillator and envelope.

```json
{
  "id": "voice",
  "type": "voice",
  "config": {
    "oscillator_type": "sine"
  }
}
```

**Config Options:**
- `oscillator_type` (optional, default: "sine"): Waveform type (sine, square, sawtooth/saw, triangle/tri)

### DAC

Digital-to-analog converter - outputs audio to speakers.

```json
{
  "id": "dac",
  "type": "dac",
  "config": {}
}
```

**Config Options:** None

## Connections

Connections define the signal flow between modules. Each connection has:

- `from`: ID of the source module
- `to`: ID of the destination module

The system currently supports linear chains (each module has at most one input and one output).

## Usage Example

### Programmatic (Imperative)

```rust
use fugue::*;

// Create modules manually
let tempo = Tempo::new(120.0);
let clock = Clock::new(sample_rate, tempo.clone());
let scale = Scale::new(Note::new(60), Mode::Dorian);
let params = MelodyParams::new(vec![0, 1, 2, 3, 4, 5, 6]);
let melody = MelodyGenerator::new(scale, params, sample_rate, tempo.clone());
let voice = Voice::new(sample_rate, OscillatorType::Sine);

// Connect manually
let audio_gen = clock.connect(melody).connect(voice);

// Start audio
let mut dac = Dac::new()?;
dac.start(audio_gen)?;
```

### Declarative

```rust
use fugue::*;

// Load patch from JSON
let patch = Patch::from_file("my_patch.json")?;

// Build and run
let dac = Dac::new()?;
let builder = PatchBuilder::new(dac.sample_rate());
let runtime = builder.build_and_run(patch)?;
let running = runtime.start()?;

// Runtime provides access to controllable parameters
running.tempo().set_bpm(140.0);
running.melody_params().set_note_duration(0.5);
```

## Extensibility

The patch format is designed for extensibility:

1. **New Module Types**: Add new module types by implementing the builder logic in `src/builder.rs`
2. **Custom Parameters**: The `ModuleConfig` struct uses `#[serde(flatten)]` to allow arbitrary additional fields
3. **Business Logic Injection**: Future versions will support hooks for custom code
4. **Real-time Input**: The architecture supports runtime parameter updates (tempo, oscillator type, etc.)

## Example Patches

See `examples/dorian_melody.json` for a complete working example.

Run it with:
```bash
cargo run --example dorian_melody_declarative
```

## Future Enhancements

- Support for parallel signal paths (mixing, routing)
- Effects modules (reverb, delay, distortion)
- Modulation sources (LFOs, envelopes)
- Control mapping and automation
- Plugin-style module loading
- Visual patch editor
