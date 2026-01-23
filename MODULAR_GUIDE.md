# Modular Architecture Guide

Fugue now implements a modular synthesis architecture inspired by Eurorack, WebAudio, and ChucK's `=>` operator.

## Core Concepts

### Signals

Like voltage in Eurorack, Fugue uses typed signals to pass information between modules:

- **`AudioSignal`**: Audio-rate samples (44.1kHz/48kHz) - the sound itself
- **`ControlSignal`**: Control-rate modulation (~1000Hz) - for LFOs, envelopes, automation
- **`ClockSignal`**: Timing information - beats, measures, phase
- **`GateSignal`**: Note on/off with velocity - like MIDI note messages
- **`TriggerSignal`**: Single-sample pulses - for percussion, events
- **`FrequencySignal`**: Pitch information in Hz - for oscillator tuning

### Modules

All components implement the `Module` trait:

```rust
pub trait Module: Send {
    fn process(&mut self) -> bool;  // Advance one sample/tick
    fn name(&self) -> &str;         // For debugging
}
```

There are two main module types:

#### Generators
Pure sources that create signals without input:

```rust
pub trait Generator<T>: Module {
    fn output(&mut self) -> T;
}
```

Examples: Clock, Oscillators, Sequencers, LFOs

#### Processors
Transform signals from input to output:

```rust
pub trait Processor<TIn, TOut>: Module {
    fn process_signal(&mut self, input: TIn) -> TOut;
}
```

Examples: Filters, Effects, Envelopes, Mixers

### Connections

Modules can be connected using the `.connect()` method, similar to patching cables in Eurorack or ChucK's `=>` operator:

```rust
// ChucK style:     osc => filter => dac
// Fugue style:     osc.connect(filter).connect(dac)

let clock = Clock::new(sample_rate, tempo);
let sequencer = Sequencer::new(...);

// Connect clock to sequencer (clock drives the sequencer)
let musical_chain = clock.connect(sequencer);
```

## Example: Clock Module

The `Clock` is a pure generator that outputs `ClockSignal`:

```rust
use fugue::*;

let sample_rate = 44100;
let tempo = Tempo::new(120.0);
let mut clock = Clock::new(sample_rate, tempo)
    .with_time_signature(4);  // 4/4 time

// Process advances time
clock.process();

// Output provides current timing information
let signal = clock.output();
println!("Beat: {}, Phase: {}", signal.beats, signal.phase);
```

### ClockSignal Fields

```rust
pub struct ClockSignal {
    pub beats: f64,           // Total beats elapsed (fractional)
    pub phase: f32,           // Phase within current beat (0.0 to 1.0)
    pub measure: u64,         // Current measure number
    pub beat_in_measure: u32, // Beat within current measure (0-indexed)
}
```

## Building Modular Chains

### Pattern 1: Generator Only

```rust
let mut clock = Clock::new(44100, Tempo::new(120.0));

loop {
    clock.process();
    let signal = clock.output();
    // Use signal...
}
```

### Pattern 2: Generator → Processor

```rust
let clock = Clock::new(44100, Tempo::new(120.0));
let divider = ClockDivider::new(2);  // Divides by 2

let mut slow_clock = clock.connect(divider);

loop {
    slow_clock.process();
    let signal = slow_clock.output();  // Half the speed
}
```

### Pattern 3: Generator → Processor → Processor

```rust
let osc = Oscillator::new(sample_rate);
let filter = LowPassFilter::new(cutoff);
let amp = Amplifier::new(0.5);

let mut voice = osc.connect(filter).connect(amp);

loop {
    voice.process();
    let audio = voice.output();
}
```

## Design Philosophy

### Eurorack Inspiration

In Eurorack:
- Each module is self-contained
- Modules communicate via CV (control voltage) and audio signals
- Patch cables connect module outputs to inputs
- Signal types are implicit but conceptually different (audio vs CV)

In Fugue:
- Each module is self-contained (implements `Module` trait)
- Modules communicate via typed signals (`AudioSignal`, `ClockSignal`, etc.)
- `.connect()` method creates signal paths
- Signal types are explicit in the type system

### WebAudio Inspiration

WebAudio's node graph:
```javascript
oscillator.connect(filter).connect(destination);
```

Fugue's equivalent:
```rust
oscillator.connect(filter).connect(output);
```

### ChucK Inspiration

ChucK's ChucK operator:
```chuck
SinOsc osc => LPF filter => dac;
```

Fugue's method chaining:
```rust
sin_osc.connect(filter).connect(dac);
```

## Benefits of This Architecture

1. **Composability**: Modules can be freely connected and rearranged
2. **Type Safety**: Signal types ensure correct connections at compile time
3. **Testability**: Each module can be tested in isolation
4. **Reusability**: Modules work anywhere they fit the signal types
5. **Clarity**: Signal flow is explicit and easy to follow
6. **Performance**: Zero-cost abstractions, compiles to efficient code

## Creating Custom Modules

### Custom Generator

```rust
pub struct WhiteNoise {
    rng: StdRng,
}

impl Module for WhiteNoise {
    fn process(&mut self) -> bool {
        true  // Always active
    }
    
    fn name(&self) -> &str {
        "WhiteNoise"
    }
}

impl Generator<AudioSignal> for WhiteNoise {
    fn output(&mut self) -> AudioSignal {
        let value = self.rng.gen_range(-1.0..1.0);
        AudioSignal::new(value)
    }
}
```

### Custom Processor

```rust
pub struct Gain {
    amount: f32,
}

impl Module for Gain {
    fn process(&mut self) -> bool {
        true
    }
}

impl Processor<AudioSignal, AudioSignal> for Gain {
    fn process_signal(&mut self, input: AudioSignal) -> AudioSignal {
        AudioSignal::new(input.value * self.amount)
    }
}
```

### Using Custom Modules

```rust
let noise = WhiteNoise::new();
let gain = Gain::new(0.3);

let mut quiet_noise = noise.connect(gain);

loop {
    quiet_noise.process();
    let audio = quiet_noise.output();
    // Send to audio output...
}
```

## Signal Flow Diagrams

### Simple Chain
```
Clock → Sequencer → Output
  │         │          │
  └─ ClockSignal ─────┘
            └─ GateSignal + FrequencySignal
```

### Parallel Processing
```
            ┌─→ Filter A ─┐
Clock → OSC ├─→ Filter B ─┼→ Mixer → Output
            └─→ Filter C ─┘
```

### Modulation
```
LFO (ControlSignal) ─┐
                     ↓
Clock → Sequencer → VCA → Output
                     ↑
                  Envelope
```

## Future Enhancements

- **Multiple inputs**: `Mixer` accepting multiple audio signals
- **Feedback loops**: With delay compensation
- **Macro modules**: Pre-built chains as single modules
- **Visual patching**: Browser-based UI for connecting modules
- **State saving**: Serialize/deserialize module graphs

## Comparison with Previous Architecture

### Before (Coupled)
```rust
let mut engine = AudioEngine::new()?;
engine.start_melody(melody_gen, tempo)?;
// Engine directly manages clock, oscillator, envelope
```

### After (Modular)
```rust
let clock = Clock::new(sample_rate, tempo);
let sequencer = Sequencer::new(scale, params);
let oscillator = Oscillator::new(sample_rate);
let envelope = Envelope::new();

let voice = clock
    .connect(sequencer)
    .connect(oscillator)
    .connect(envelope);

// Voice is self-contained and composable
```

---

The modular architecture makes Fugue more flexible, reusable, and closer to the mental model of modular synthesis while maintaining Rust's type safety and zero-cost abstractions.
