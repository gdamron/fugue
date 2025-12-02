# Getting Started with Fugue

## Installation

Ensure you have Rust installed (1.70+):

```bash
cargo --version
```

## Running the Example

The quickest way to experience Fugue is to run the Dorian melody example:

```bash
cargo run --example dorian_melody
```

This will generate and play an infinite, randomly generated melody in D Dorian mode.

## Interactive Controls

Once the melody is playing, you can modify it in real-time:

### Scale Degree Control
- `1` through `7`: Toggle individual scale degrees
  - `1` = Root (D)
  - `2` = Second (E)
  - `3` = Minor third (F)
  - `4` = Fourth (G)
  - `5` = Fifth (A)
  - `6` = Sixth (B)
  - `7` = Minor seventh (C)

### Oscillator Types
- `s`: Sine wave (smooth, pure tone)
- `w`: Sawtooth wave (bright, buzzy)
- `t`: Triangle wave (hollow, mellow)
- `q`: Square wave (hollow, retro)

### Tempo & Timing
- `+`: Increase tempo by 10 BPM
- `-`: Decrease tempo by 10 BPM
- `f`: Faster notes (shorter duration)
- `n`: Slower notes (longer duration)

### Presets
- `r`: Emphasize root and fifth (more stable sound)

### Exit
- `x`: Stop playback and exit

## Example Session

```
> s           # Switch to sine wave
> 3           # Remove the minor third
> 7           # Remove the minor seventh
> +           # Speed up
> +           # Speed up more
> r           # Emphasize root and fifth
> w           # Switch to sawtooth
> x           # Exit
```

## Understanding the Code

### Basic Setup

```rust
use fugue::*;

// Create tempo
let tempo = Tempo::new(120.0);

// Create scale (D Dorian)
let root = Note::new(62);  // MIDI note 62 = D4
let scale = Scale::new(root, Mode::Dorian);

// Set up melody parameters
let allowed_degrees = vec![0, 1, 2, 3, 4, 5, 6];
let params = MelodyParams::new(allowed_degrees);

// Generate melody
let melody_gen = MelodyGenerator::new(scale, params.clone());

// Start audio
let mut engine = AudioEngine::new()?;
engine.start_melody(melody_gen, tempo.clone())?;
```

### Live Parameter Updates

All parameters can be updated while audio is playing:

```rust
// Change oscillator type
params.set_oscillator_type(OscillatorType::Square);

// Modify scale degrees
params.set_allowed_degrees(vec![0, 2, 4]);  // Triadic melody

// Adjust note weights (make some notes more likely)
params.set_note_weights(vec![3.0, 1.0, 2.0]);  // Root emphasized

// Change note duration (in beats)
params.set_note_duration(0.5);  // Half notes

// Update tempo
tempo.set_bpm(140.0);
```

## Next Steps

- Explore different modes: `Mode::Ionian`, `Mode::Phrygian`, `Mode::Lydian`, etc.
- Experiment with different root notes (MIDI 0-127)
- Try different note weight distributions
- Create your own examples combining multiple generators

## Troubleshooting

### No audio output
- Check your system audio settings
- Ensure no other application is blocking audio
- Try running with `--release` for better performance: `cargo run --example dorian_melody --release`

### Audio glitches
- Increase buffer size (future feature)
- Use release mode for better performance
- Close other audio applications
