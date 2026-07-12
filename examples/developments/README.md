# Voice Development Library

Reusable voice developments. Register each one under `developments` and instantiate it like a normal module — every preset exposes the same interface so they are interchangeable:

- **Inputs:** `frequency`, `gate`, `pedal`
- **Output:** `audio`

Each voice routes its `gate` through an internal `sustain` module before the
envelope, and exposes the module's `pedal` port. While the pedal gate is
high, a key-up leaves the envelope's gate open, so the note keeps evolving on
its own terms: the struck voices (piano, marimba, pluck — envelope sustain
level 0) ring out on their natural decay, while vibraphone and pad hold their
plateaus. Pedal-up releases held notes over the envelope's release time.

For polyphony, build a bank: a `divisi` module fans a monophonic line across
N explicit voice instances (latching pitch/velocity per note, stealing the
oldest voice when full), a mixer sums them, and the bank's `pedal` input fans
out to every voice by ordinary connections — so a development can just as
well contain two instruments with independent pedals.

## Presets

| File | Character | Exposed controls |
| --- | --- | --- |
| `piano.json` | Struck sawtooth through a lowpass that opens on attack and closes as the note decays — bright transient into a warm tail. Low sustain; no vibrato. | `decay`, `sustain`, `release`, `brightness_peak` |
| `marimba.json` | Short, woody struck tone. Triangle oscillator into a bandpass with a fast filter-sweep envelope, sustain at zero so notes cut off cleanly. | `decay`, `tone`, `resonance` |
| `vibraphone.json` | Sine tone with long ring, gentle 5 Hz tremolo, and a narrow bandpass for bell-like colour. Slow attack, high sustain. | `attack`, `release`, `tremolo_depth`, `tremolo_rate`, `brightness` |
| `pluck.json` | Very short square-wave pluck with an aggressive lowpass sweep. The envelope decays in ~70 ms — tight and percussive. | `decay`, `brightness`, `bite` |
| `pad.json` | Slow-attack sawtooth with a drifting lowpass. A 0.18 Hz LFO keeps the tone gently evolving through the sustain. | `attack`, `release`, `warmth`, `motion` |

## Usage

```json
{
  "developments": [
    { "name": "piano", "path": "examples/developments/piano.json" }
  ],
  "modules": [
    { "id": "p1", "type": "piano" },
    { "id": "p2", "type": "piano" }
  ]
}
```

Paths are resolved relative to the loading invention's location. When loading from `examples/`, use `developments/piano.json`; when loading from `examples/developments/`, use just `piano.json`.

## Verification patch

`voice_library_trio.json` drives three voices from different clock subdivisions and registers to produce a simple bass + melody + pad arrangement:

- **Bass** (pluck) — A2 register, quarter notes, panned slightly left
- **Melody** (piano) — A4 register, eighth notes, panned slightly right
- **Pad** — A3 register, half notes, centre

Run it with:

```
cargo run --example examples -- --example developments/voice_library_trio.json
```
