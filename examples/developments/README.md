# Voice Development Library

These reusable developments are meant to be registered under `developments` and instantiated like normal modules. Every preset exposes the same shared ports:

- Inputs: `frequency`, `gate`
- Output: `audio`

## Presets

| File | Character | Exposed controls |
| --- | --- | --- |
| `piano.json` | Bright filtered saw with gentle pitch drift and a moderate decay. | `attack`, `decay`, `release`, `brightness`, `detune` |
| `marimba.json` | Short, woody struck voice with a fast bandpass sweep. | `decay`, `tone`, `resonance` |
| `vibraphone.json` | Mellow sustained bell tone with a slow tremolo shimmer. | `attack`, `release`, `tremolo_depth`, `tremolo_rate`, `brightness` |
| `pluck.json` | Tight, sharp pluck with a quick lowpass snap. | `decay`, `brightness`, `bite` |
| `pad.json` | Slow, sustained pad with gentle filter motion. | `attack`, `release`, `warmth`, `motion` |

## Verification Patch

`voice_library_trio.json` registers three of the presets and mixes them together so the library can be smoke-tested as multiple simultaneous instances inside one invention.
