# Fugue

A system for composing algorithmic and generative music.

## Features

- 🎵 **Cross-platform**: Designed to run everywhere
- 📄 **Inventions**: A declarative document format for describing a composition
- 🎚️ **Live control**: Update scales, rhythms, and synthesis parameters in real-time
- 🤖 **MCP Server**: Collaborate with LLM agents

## Install

One-line install (macOS and Linux), no Rust toolchain required. This downloads
the prebuilt `fugue` and `fugue-mcp` binaries into `~/.fugue/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/gdamron/fugue/main/install.sh | sh
```

Windows PowerShell install:

```powershell
iwr https://raw.githubusercontent.com/gdamron/fugue/main/install.ps1 -useb | iex
```

Then add `~/.fugue/bin` to your `PATH` (the installer prints the exact line) and
register the MCP server with Claude Code:

```sh
claude mcp add fugue ~/.fugue/bin/fugue-mcp
```

`fugue-mcp` auto-spawns `fugue serve` from your `PATH`, so installing both
binaries together is all that's required to play audio.

Overrides: `FUGUE_BIN_DIR` (install location), `FUGUE_VERSION` (pin a specific
release tag, default `latest`).

Prebuilt binaries are published for **macOS arm64**, **Linux x86_64 / arm64**
(glibc), and **Windows x86_64**. Linux binaries dynamically link ALSA — install `libasound2`
(Debian/Ubuntu: `sudo apt-get install libasound2`) if it is not already present.
**Intel Macs** are not yet prebuilt; build from source for those.

## Quick Start

### Run the Examples

Fugue ships with one example runner that can play the curated JSON inventions in `examples/`.

Start the interactive selector:
```bash
cargo run --example examples
```

Run a specific invention directly by JSON filename:
```bash
cargo run --example examples -- --example simple_tone.json
```

Nested curated examples work the same way:
```bash
cargo run --example examples -- --example developments/voice_library_trio.json
```

Current playable examples include:

- `simple_tone.json`
- `modular_adsr_melody.json`
- `filter_envelope.json`
- `filter_lfo_wah.json`
- `lfo_vibrato.json`
- `lfo_tremolo_sync.json`
- `mixer_voices.json`
- `step_sequencer.json`
- `control_scheduler.json`
- `development_file_patch.json`
- `development_inline_patch.json`
- `developments/voice_library_trio.json`

### Development Examples

JSON examples for the new development format live in `examples/`:

- `examples/development_voice.json`: a reusable development document
- `examples/development_inline_patch.json`: registers a development inline
- `examples/development_file_patch.json`: registers a development from a file path

### Run an Invention

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


## CLI and REPL

The Fugue command-line host lives in the separate `fugue-cli` repository. It
provides playback, rendering, daemon, and interactive REPL commands while this
repository stays focused on the core library and runtime.

From a sibling checkout:

```bash
cd ../fugue-cli
cargo run -- repl
```

### Example Session

```
fugue> new My Jam
Created invention 'My Jam' with DAC.
fugue> add clock1 clock
Added clock 'clock1'.
fugue> add melody1 melody
Added melody 'melody1'.
fugue> add osc1 oscillator
Added oscillator 'osc1'.
fugue> connect clock1 gate melody1 gate
Connected clock1:gate -> melody1:gate
fugue> connect melody1 frequency osc1 frequency
Connected melody1:frequency -> osc1:frequency
fugue> connect osc1 audio dac audio
Connected osc1:audio -> dac:audio
fugue> set clock1 bpm 140
clock1.bpm = 140
fugue> controls clock1
clock1:
    bpm                 120  (60 - 300, default: 120) Tempo in beats per minute
fugue> types
(lists all module types with ports and controls)
fugue> quit
```

Type `help` for the full command reference.

## Browser Playback

The JavaScript browser player is packaged separately as `@ilusiv/fugue-js`.
This repository owns the Rust engine and wasm exports that package consumes.

## MCP Server (AI-Driven Composition)

The MCP server now lives in the separate
[`fugue-mcp`](https://github.com/gdamron/fugue-mcp) repository. It exposes the
runtime API as tools and talks to `fugue serve` over the shared runtime RPC
protocol.
