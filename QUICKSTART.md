# Fugue Quickstart

Get started with Fugue in 5 minutes!

## Installation

```bash
# Clone or navigate to the project
cd fugue

# Build the library
cargo build --release
```

## Run the Demo

```bash
cargo run --example dorian_melody --release
```

You'll hear a randomly generated melody in the Dorian mode!

## Try These Commands

Once the demo is running, try:

```
s      # Switch to sine wave (smooth sound)
w      # Switch to sawtooth (bright sound)
q      # Switch to square wave (retro 8-bit sound)
+      # Speed up the tempo
r      # Emphasize root and fifth notes
3      # Remove the minor third from the scale
7      # Remove the minor seventh
1      # Add them back
x      # Exit
```

## Your First Fugue Program

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
    
    // Play for 10 seconds
    thread::sleep(Duration::from_secs(10));
    
    Ok(())
}
```

Run it:

```bash
cargo run --example my_melody --release
```

## Key Concepts

### Notes
```rust
let c4 = Note::new(60);         // Middle C (MIDI note 60)
let a4 = Note::new(69);         // Concert A (440 Hz)
let freq = a4.frequency();      // Get frequency: 440.0
```

### Scales and Modes
```rust
// Create different scales
let major = Scale::new(Note::new(60), Mode::Ionian);
let dorian = Scale::new(Note::new(62), Mode::Dorian);
let minor = Scale::new(Note::new(57), Mode::Aeolian);

// Get notes from scale
let root = scale.get_note(0);   // First degree
let fifth = scale.get_note(4);  // Fifth degree
```

### Tempo
```rust
let tempo = Tempo::new(120.0);  // 120 BPM
tempo.set_bpm(140.0);           // Change it live!
```

### Oscillators
```rust
params.set_oscillator_type(OscillatorType::Sine);
params.set_oscillator_type(OscillatorType::Square);
params.set_oscillator_type(OscillatorType::Sawtooth);
params.set_oscillator_type(OscillatorType::Triangle);
```

### Melody Control
```rust
// Which scale degrees can be played
params.set_allowed_degrees(vec![0, 2, 4]);  // Triadic

// How often each degree is played (relative weights)
params.set_note_weights(vec![3.0, 1.0, 2.0]);  // Root emphasized

// Note duration in beats
params.set_note_duration(0.5);  // Half notes
params.set_note_duration(0.25); // Quarter notes
```

## Common Patterns

### Pentatonic Scale
```rust
// Use only pentatonic degrees from any mode
let params = MelodyParams::new(vec![0, 2, 4, 5, 7]);
```

### Bass Line
```rust
// Low notes, emphasized root
let root = Note::new(36);  // C2 (low C)
let scale = Scale::new(root, Mode::Dorian);
let params = MelodyParams::new(vec![0, 4]);  // Root and fifth
params.set_note_weights(vec![3.0, 1.0]);
params.set_note_duration(1.0);  // Whole notes
```

### Fast Arpeggio
```rust
let params = MelodyParams::new(vec![0, 2, 4, 7]);  // Triad + octave
params.set_note_duration(0.125);  // Eighth notes
tempo.set_bpm(160.0);
```

## Modes Reference

```rust
Mode::Ionian      // Major - happy, bright
Mode::Dorian      // Minor - jazzy, balanced
Mode::Phrygian    // Minor - Spanish, exotic
Mode::Lydian      // Major - dreamy, floating
Mode::Mixolydian  // Major - bluesy, dominant
Mode::Aeolian     // Natural minor - sad, dark
Mode::Locrian     // Diminished - unstable, tense
```

## Next Steps

1. **Experiment with modes**: Try all seven modes with different root notes
2. **Create patterns**: Use weighted probabilities to create recurring motifs
3. **Combine parameters**: Change multiple things at once
4. **Read the docs**: Check out `MUSIC_REFERENCE.md` for detailed theory

## Troubleshooting

**No sound?**
- Check system volume
- Ensure no other app is blocking audio
- Try `--release` mode for better performance

**Audio glitches?**
- Close other audio applications
- Use release builds: `cargo run --example X --release`

**Want to learn more?**
- `README.md` - Project overview
- `GETTING_STARTED.md` - Detailed guide
- `MUSIC_REFERENCE.md` - Music theory reference
- `ARCHITECTURE.md` - Technical deep dive

---

Happy composing! 🎵
