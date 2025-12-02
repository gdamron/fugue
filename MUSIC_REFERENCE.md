# Musical Concepts in Fugue

## Modes

Fugue supports all seven diatonic modes. Each mode has a unique character and interval pattern.

### Available Modes

| Mode | Character | Intervals (semitones from root) |
|------|-----------|--------------------------------|
| Ionian | Major, happy, bright | 0, 2, 4, 5, 7, 9, 11 |
| Dorian | Minor, jazzy, balanced | 0, 2, 3, 5, 7, 9, 10 |
| Phrygian | Minor, Spanish, exotic | 0, 1, 3, 5, 7, 8, 10 |
| Lydian | Major, dreamy, floating | 0, 2, 4, 6, 7, 9, 11 |
| Mixolydian | Major, bluesy, dominant | 0, 2, 4, 5, 7, 9, 10 |
| Aeolian | Natural minor, sad | 0, 2, 3, 5, 7, 8, 10 |
| Locrian | Diminished, unstable | 0, 1, 3, 5, 6, 8, 10 |

### Example: D Dorian

```
D  E  F  G  A  B  C  D
1  2  ♭3 4  5  6  ♭7 8
```

## MIDI Notes

Fugue uses MIDI note numbers (0-127) to represent pitches.

### Common Notes

| Note | MIDI Number | Frequency (Hz) |
|------|-------------|----------------|
| C4 (Middle C) | 60 | 261.63 |
| D4 | 62 | 293.66 |
| E4 | 64 | 329.63 |
| A4 (Concert pitch) | 69 | 440.00 |
| C5 | 72 | 523.25 |

### Calculating MIDI Notes

```rust
// Create note from MIDI number
let middle_c = Note::new(60);

// Get frequency
let freq = middle_c.frequency();  // 261.63 Hz

// Create from frequency
let a440 = Note::from_frequency(440.0);
```

## Oscillators

Fugue provides four basic waveform oscillators, each with a distinct sound character.

### Waveform Types

#### Sine Wave
```
     ___
    /   \
   /     \___
```
- **Sound**: Pure, smooth, no overtones
- **Use**: Sub bass, pure tones, FM synthesis carrier
- **Character**: Gentle, soft

#### Square Wave
```
   ___     ___
  |   |   |   |
__|   |___|   |__
```
- **Sound**: Hollow, retro, rich in odd harmonics
- **Use**: 8-bit game music, bass lines, leads
- **Character**: Buzzy, vintage

#### Sawtooth Wave
```
   /|   /|   /|
  / |  / |  / |
 /  | /  | /  |
```
- **Sound**: Bright, rich in all harmonics
- **Use**: Pad sounds, brass, strings
- **Character**: Bright, full

#### Triangle Wave
```
   /\    /\
  /  \  /  \
 /    \/    \
```
- **Sound**: Mellow, soft, odd harmonics only
- **Use**: Flutes, soft leads, bell tones
- **Character**: Hollow, gentle

## Time and Tempo

### Tempo (BPM)
- Beats per minute
- Example: 120 BPM = 2 beats per second
- Range: typically 40-200 BPM

### Note Duration
- Measured in beats
- `1.0` = whole note
- `0.5` = half note
- `0.25` = quarter note
- `0.125` = eighth note

### Samples
- Fundamental unit of digital audio
- Sample rate: typically 44100 or 48000 Hz
- 44100 samples = 1 second of audio

## Scale Degrees

Scale degrees are positions within a scale, starting from 0.

```rust
let scale = Scale::new(Note::new(60), Mode::Dorian);

scale.get_note(0);  // Root (C)
scale.get_note(1);  // Second (D)
scale.get_note(2);  // Minor third (E♭)
scale.get_note(7);  // Root, one octave up
```

## Weighted Probability

Control which notes appear more frequently:

```rust
let params = MelodyParams::new(vec![0, 1, 2, 3, 4, 5, 6]);

// Equal probability (default)
params.set_note_weights(vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);

// Emphasize root and fifth
params.set_note_weights(vec![3.0, 1.0, 1.0, 1.0, 2.0, 1.0, 1.0]);

// Heavily favor root
params.set_note_weights(vec![10.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
```

Weights are relative - `[2.0, 1.0]` is the same as `[20.0, 10.0]`.

## Envelope

The simple amplitude envelope in Fugue:

```
  Attack     Sustain      Release
   _______________
  /               \
 /                 \
/                   \___
```

- **Attack**: 10% of note duration
- **Sustain**: 80% of note duration
- **Release**: 10% of note duration

This prevents clicks and creates more natural-sounding notes.
