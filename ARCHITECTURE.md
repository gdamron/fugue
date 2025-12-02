# Fugue Architecture

## System Overview

```
┌─────────────────────────────────────────────────────────────┐
│                        Application                          │
│                     (dorian_melody.rs)                      │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
        ┌─────────────────────────────────────────┐
        │         Fugue Library (lib.rs)          │
        └─────────────────────────────────────────┘
                              │
        ┌─────────────────────┴─────────────────────┐
        │                                           │
        ▼                                           ▼
┌──────────────────┐                    ┌──────────────────┐
│  Music Theory    │                    │  Audio System    │
│   (scale.rs)     │                    │   (audio.rs)     │
│                  │                    │                  │
│  • Modes         │                    │  • AudioEngine   │
│  • Scales        │                    │  • cpal          │
│  • Notes         │                    │  • Streaming     │
└──────────────────┘                    └──────────────────┘
        │                                           │
        │                                           │
        ▼                                           ▼
┌──────────────────┐                    ┌──────────────────┐
│  Composition     │                    │  Synthesis       │
│ (sequencer.rs)   │                    │ (synthesis.rs)   │
│                  │                    │                  │
│  • Melody Gen    │◄───────────────────│  • Oscillators   │
│  • Parameters    │                    │  • Filters       │
│  • Probability   │                    │  • Waveforms     │
└──────────────────┘                    └──────────────────┘
        │                                           │
        │                                           │
        └───────────────┬───────────────────────────┘
                        │
                        ▼
                ┌──────────────────┐
                │   Time System    │
                │    (time.rs)     │
                │                  │
                │  • Clock         │
                │  • Tempo         │
                │  • Timing        │
                └──────────────────┘
```

## Data Flow

### Audio Generation Pipeline

```
User Input
    │
    ▼
MelodyParams (Arc<Mutex<>>)
    │
    ├─► allowed_degrees ──┐
    ├─► note_weights      │
    ├─► note_duration     │
    └─► oscillator_type   │
                          │
                          ▼
                   MelodyGenerator
                          │
                          ├─► Random Selection
                          ├─► Scale Mapping
                          └─► Note Generation
                                    │
                                    ▼
                                  Note
                                    │
                                    ├─► MIDI Number
                                    └─► Frequency (Hz)
                                              │
                                              ▼
                                        Oscillator
                                              │
                                              ├─► Waveform Generation
                                              ├─► Phase Accumulation
                                              └─► Sample Output
                                                        │
                                                        ▼
                                                    Envelope
                                                        │
                                                        └─► Attack/Sustain/Release
                                                                  │
                                                                  ▼
                                                             Audio Stream
                                                                  │
                                                                  └─► Hardware (cpal)
```

## Component Interaction

### Real-time Parameter Updates

```
Main Thread                          Audio Thread
─────────────                        ─────────────

params.set_oscillator_type()
    │
    ├─► Arc::clone()
    │   Mutex::lock()
    │   *value = new_value
    │
    │                                MelodyGenerator
    │                                     │
    │                                     ├─► params.get_oscillator_type()
    │                                     │   Arc::clone()
    │                                     │   Mutex::lock()
    │                                     │   read value
    │                                     │
    │                                     ▼
    │                                oscillator.set_type()
    │                                     │
    │                                     └─► Audio continues seamlessly

tempo.set_bpm()
    │
    └─► Similar pattern for tempo changes
```

### Clock and Timing

```
Sample Loop (44100 Hz)
    │
    ├─► clock.tick()
    │   sample_count++
    │
    ├─► Calculate: samples_per_note
    │   = (sample_rate * 60 / BPM) * note_duration
    │
    ├─► if samples_since_note >= samples_per_note:
    │       │
    │       ├─► Generate new note
    │       ├─► Update oscillator frequency
    │       └─► Reset counter
    │
    └─► Generate audio sample
```

## Module Responsibilities

### time.rs
- **Clock**: Sample-accurate timing
- **Tempo**: BPM management with thread-safe updates
- **Conversions**: Samples ↔ Beats ↔ Time

### synthesis.rs
- **Oscillator**: Waveform generation (sine, square, saw, triangle)
- **Filter**: Basic audio filtering (prepared for future use)
- **Phase accumulation**: Continuous waveform generation

### scale.rs
- **Mode**: Interval patterns for musical modes
- **Note**: MIDI ↔ Frequency conversion
- **Scale**: Degree-to-note mapping

### sequencer.rs
- **MelodyParams**: Thread-safe parameter container
- **MelodyGenerator**: Probabilistic note selection
- **Weighted random**: Customizable note distributions

### audio.rs
- **AudioEngine**: cpal integration
- **Sample format handling**: F32, I16, U16 support
- **Stream management**: Audio device lifecycle
- **Real-time audio callback**: Per-sample processing

## Thread Safety

All shared state uses `Arc<Mutex<T>>`:

```rust
pub struct MelodyParams {
    pub allowed_degrees: Arc<Mutex<Vec<usize>>>,
    pub note_weights: Arc<Mutex<Vec<f32>>>,
    pub note_duration: Arc<Mutex<f32>>,
    pub oscillator_type: Arc<Mutex<OscillatorType>>,
}
```

This enables:
- **Main thread**: User input → Parameter updates
- **Audio thread**: Lock → Read → Generate audio
- **No blocking**: Lock held only for brief reads/writes

## Key Algorithms

### Weighted Random Selection

```rust
// Given: allowed_degrees = [0, 2, 4], weights = [3.0, 1.0, 2.0]
// Total weight = 6.0

random_value = rand() * 6.0  // e.g., 4.2

// Accumulate weights:
// 0: 0.0 ─► 3.0  (if random_value < 3.0, return degree 0)
// 2: 3.0 ─► 4.0  (if random_value < 4.0, return degree 2)
// 4: 4.0 ─► 6.0  (if random_value < 6.0, return degree 4) ✓
```

### Envelope Generation

```rust
// For a note duration of 1000 samples:
attack_samples = 100  // 10%
release_samples = 100 // 10%
sustain_samples = 800 // 80%

if sample < attack_samples:
    amplitude = sample / attack_samples
else if sample > (total - release_samples):
    amplitude = (total - sample) / release_samples
else:
    amplitude = 1.0
```

## Performance Characteristics

- **Real-time audio**: ~10ms latency (cpal default)
- **Lock contention**: Minimal (brief mutex holds)
- **Memory**: ~1KB per parameter set
- **CPU**: <1% single core (release build)
- **Sample rate**: 44.1kHz or 48kHz (device dependent)

## Extensibility Points

1. **New oscillator types**: Add to `OscillatorType` enum
2. **New modes**: Add to `Mode` enum with interval pattern
3. **Effects chain**: Insert between oscillator and output
4. **Multiple voices**: Parallel `MelodyGenerator` instances
5. **Rhythm patterns**: Extend timing logic in audio callback
6. **WebAssembly**: Swap `cpal` for Web Audio API

## Design Patterns

- **Builder pattern**: Parameter construction
- **Strategy pattern**: Oscillator type selection
- **Observer pattern**: Parameter updates
- **Facade pattern**: AudioEngine simplifies cpal
- **Immutable data**: Arc-wrapped shared state

---

This architecture balances:
- **Simplicity**: Easy to understand and extend
- **Flexibility**: Components can be reconfigured
- **Performance**: Real-time audio with minimal overhead
- **Safety**: Thread-safe parameter updates
