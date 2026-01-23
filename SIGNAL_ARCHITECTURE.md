# Signal Architecture

## Overview

Fugue uses a simple, Eurorack-inspired signal architecture with two fundamental types:

1. **Audio** - Real-time audio-rate signals (the "patch cables")
2. **Control** - Human input and parameter changes (the "knobs and switches")

## Audio Signals

`Audio` represents any signal flowing through the modular chain at audio rate (e.g., 44,100 samples per second).

### What Audio Carries

In Eurorack modular synthesis, everything flowing through patch cables is voltage. Similarly, in Fugue, everything flowing through modules is `Audio`:

- **Sound waveforms** - actual audio content
- **Control Voltages (CV)** - pitch, modulation, etc.
- **Gates** - note on/off with velocity (0.0 = off, >0.0 = on)
- **Triggers** - single-sample pulses
- **Envelopes** - ADSR, ASR, etc.
- **LFOs** - low-frequency modulation

### Usage

```rust
use fugue::Audio;

// Create audio signals
let silence = Audio::silence();              // 0.0
let sound = Audio::new(0.5);                // Arbitrary value
let gate = Audio::gate(true, 0.8);          // Gate on with 80% velocity
let pitch = Audio::from_midi(60);            // Middle C as frequency

// Access the value
let value: f32 = audio.value;
```

### Design Philosophy

Just like in Eurorack where you can patch anything to anything (audio to CV, LFO to pitch, etc.), `Audio` is intentionally generic. The meaning comes from context and how modules interpret the signal, not from the type itself.

## Control Signals

`Control<T>` represents human input and parameter changes that happen outside the real-time audio processing.

### What Control Represents

- **Knob positions** - filter cutoff, resonance, etc.
- **Button states** - on/off switches
- **Selection** - waveform type, scale selection
- **Key presses** - UI commands
- **Parameter automation** - pre-programmed changes
- **MIDI CC** - MIDI controller data

### Usage

```rust
use fugue::Control;

// Create controls
let tempo = Control::new(120.0);
let osc_type = Control::new(OscillatorType::Sine);

// Read values (thread-safe)
let current_tempo = tempo.get();              // Copy for simple types

// Update values (from any thread)
tempo.set(140.0);

// Work with complex types
tempo.with(|t| println!("Tempo: {}", t));     // Read with closure
tempo.modify(|t| *t += 10.0);                // Modify with closure

// Share with modules (Arc<Mutex<T>> internally)
let shared = tempo.inner();
```

### Thread Safety

`Control` uses `Arc<Mutex<T>>` internally, making it safe to read from the audio thread while the UI thread updates it. This allows live parameter changes without audio dropouts.

## Examples

### Simple Oscillator

```rust
// Audio-rate processing
let mut osc = Oscillator::new(sample_rate, OscillatorType::Sine);
osc.set_frequency(440.0);
osc.process();
let audio: Audio = osc.output();  // Audio signal out
```

### Live Parameter Control

```rust
// Control values (not audio-rate)
let osc_type = Control::new(OscillatorType::Sine);
let cutoff = Control::new(1000.0);

// In audio callback
loop {
    // Read controls (thread-safe)
    oscillator.set_type(osc_type.get());
    filter.set_cutoff(cutoff.get());
    
    // Process audio
    oscillator.process();
    let audio = oscillator.output();
}

// From UI thread
osc_type.set(OscillatorType::Sawtooth);  // Live update!
```

### Modular Chain

```rust
// Build signal chain
let clock = Clock::new(sample_rate, tempo);        // Generates ClockSignal
let sequencer = MelodyGenerator::new(...);         // ClockSignal → NoteSignal  
let voice = Voice::new(sample_rate, osc_type);     // NoteSignal → Audio

// Connect modules
let audio_gen = clock.connect(sequencer).connect(voice);

// Output to DAC
let mut dac = Dac::new()?;
dac.start(audio_gen)?;  // Plays Audio through speakers
```

## Signal Types Reference

### Core Types

| Type | Rate | Purpose | Example Values |
|------|------|---------|----------------|
| `Audio` | Audio (44.1kHz) | Real-time signal | -1.0 to 1.0 |
| `Control<T>` | Event-driven | User input | Any type T |

### Specialized Audio Signals

These are still `Audio` at their core, but with semantic meaning:

| Interpretation | Range | Usage |
|----------------|-------|-------|
| Sound | -1.0 to 1.0 | Audio waveforms |
| Gate | 0.0 = off, >0.0 = velocity | Note triggers |
| CV | 0.0 to 1.0 typical | Modulation |
| Frequency | Hz value | Pitch (e.g., 440.0) |

### Compound Types

Some modules need structured data:

| Type | Fields | Purpose |
|------|--------|---------|
| `ClockSignal` | beats, phase, measure | Timing info |
| `FrequencySignal` | hz | Pitch information |
| `NoteSignal` | gate (Audio), frequency | Musical note |

These are audio-rate types that carry multiple pieces of information.

## Design Rationale

### Why Two Types?

1. **Clarity** - Separates real-time processing from parameter changes
2. **Performance** - Audio signals are lightweight copies, Controls are thread-safe references
3. **Flexibility** - Like Eurorack, Audio can represent anything
4. **Safety** - Type system prevents mixing audio processing with parameter updates

### Eurorack Inspiration

In Eurorack modular synthesis:
- Everything through patch cables is voltage (~ our `Audio`)
- Knobs and switches are manual controls (~ our `Control`)
- You can patch anything to anything
- Context determines meaning

Fugue follows the same philosophy in software.

## Migration Notes

For backward compatibility, these legacy type aliases exist:
- `AudioSignal` = `Audio`
- `ControlSignal` = `Audio`  
- `GateSignal` = `Audio`
- `TriggerSignal` = `Audio`

New code should use `Audio` directly.

---

**Summary**: Everything flowing through modules is `Audio`. Everything controlled by humans is `Control<T>`. Simple, flexible, and Eurorack-inspired.
