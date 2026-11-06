# Fugue

A system for composing algorithmic and generative music.

## Features

- 🎵 **Cross-platform**: Designed to run everywhere
- 📄 **Inventions**: A declarative document format for describing a composition
- 🎚️ **Live control**: Update scales, rhythms, and synthesis parameters in real-time
- 🤖 **MCP Server**: Collaborate with LLM agents

See the [module discovery contract](src/rpc/DISCOVERY.md) for compact type
indexes, configured inspection, live metadata, and response limits.

## Install

This repository is the Fugue core: the Rust library and runtime that play
inventions. The Fugue CLI and MCP app are built and released by the Fugue
product, which documents how to install them; their files are published on this
repository's releases alongside the library archives.

### Use the library

Add the crate as a git dependency (the `fugue` name on crates.io belongs to a
different project). Releases are tagged `v<year>.<month>.<patch>`; add
`tag = "v<version>"` to pin one.

```toml
[dependencies]
fugue = { git = "https://github.com/gdamron/fugue" }
```

Optional cargo features, all off by default:

| Feature | Adds |
|---|---|
| `ffi` | The C ABI over the render engine (`include/fugue.h`), used by the native, iOS and Android archives. |
| `plugins` | Hosting for WebAssembly component modules (see [Wasm modules](docs/wasm-modules.md)). |
| `rpc-schema` | JSON Schema generation for the runtime RPC types. |
| `score-import` | Importing scores from MusicXML. |
| `spectrogram` | Spectrogram analysis of running audio. |

See [Run an Invention](#run-an-invention) for loading and playing one from Rust.

### Build and run the examples

From a checkout with a Rust toolchain, `cargo run --example examples` plays the
curated inventions in `examples/` (see [Quick Start](#quick-start)). Linux
builds need the ALSA development headers (Debian/Ubuntu:
`sudo apt-get install libasound2-dev pkg-config`).

### Release archives

Each [GitHub release](https://github.com/gdamron/fugue/releases) carries the
library archives this repository's release workflow builds: native builds
named `fugue-<version>-<target>.tar.gz` (macOS arm64, Linux x86_64, Windows
x86_64), plus iOS (`-ios-xcframework`), Android (`-android`) and wasm (`-web`)
archives. Every library archive includes `THIRD_PARTY_LICENSES.txt`, the
license notices for the dependencies built into it; keep that file with any
copy you redistribute. `SHA256SUMS.txt` on the release lists the checksums.

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

Many examples are generative: a melody, chance or noise module with no seed
plays a different performance every run. Pass `--seed <n>` to repeat one:
every unseeded random source then plays a stream derived from `n` and its
module id, and modules with a seed of their own keep it.
```bash
cargo run --example examples -- --example chance_groove.json --seed 7
```
Offline renders take the same seed through `RenderEngine::set_seed`, set
before loading the invention; test renders can also stub agent backends with
`RenderEngine::stub_agent_backends`, so no service or local process is
contacted. `code` and `agent` modules run on wall-clock time, so an invention
using them (such as `in_c.json`) still varies under a seed.

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

A development exposes its interface by `name`: each `inputs` entry routes a
name `to` an inner module's `to_port`, each `outputs` entry `from` an inner
`from_port`, and each `controls` entry to an inner `module`'s `control`.
Listing one control name more than once fans it out to every target.
Exposed names follow the interface conventions: the musical name of the
gesture (`brightness`), not the inner module's key (`cutoff`).

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
        "degrees": [0, 1, 2, 3, 4, 5, 6]
      }
    },
    {
      "id": "voice",
      "type": "voice",
      "config": { "waveform": "sine" }
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
running.set_control("melody", "note_weights", ControlValue::String("[1, 0.5, 1]".into()))?;
```

### Audio Output

Live playback opens the system's default output device. These environment
variables change that for any host built on the core (`fyug serve` and `fyug
play`, named `fugue` in older installs, or your own program). Set them in the environment that starts the
process: a daemon that is already running keeps the device it started with.

| Variable | Effect |
|---|---|
| `FUGUE_OUTPUT_DEVICE` | Play to the output device with exactly this name instead of the default, e.g. `FUGUE_OUTPUT_DEVICE="BlackHole 16ch"`. Unset or empty means the default output. |
| `FUGUE_AUDIO_DIAGNOSTICS_LOG` | When set, log each audio callback that misses its deadline to stderr. |

The device name must match exactly, including case and spaces; if several
devices share it, the first is used. A name that matches no device is an
error, raised when playback starts, that lists the devices available, and
nothing plays: Fugue never falls back to the default output. If the named
device goes away while playing, Fugue waits for that device to return rather
than moving to another one.

#### Capturing audio as an agent

An agent that needs to hear or measure what it plays should send it to a
virtual loopback device such as
[BlackHole](https://github.com/ExistentialAudio/BlackHole), named exactly,
and record from that same device:

```sh
FUGUE_OUTPUT_DEVICE="BlackHole 16ch" fyug serve --rpc 127.0.0.1:<port> &
# Fugue writes the first two channels; keep exactly those.
sox -t coreaudio "BlackHole 16ch" capture.wav remix 1 2 trim 0 5
```

Never capture through the default output: it is the listener's speakers or
headphones. Name the loopback device every time, so that if it is missing the
run fails with the device list instead of playing out loud. Avoid names that
are themselves aliases for the system output, such as ALSA's `default` or
`pipewire` on Linux. Start the host after the loopback device exists, so the
graph is built at that device's sample rate. To list the device names without
opening any device, start playback with the variable set to a name that
cannot exist and read the list in the error.


## CLI and REPL

The `fugue` command-line host is built and released by the Fugue product (see
[Install](#install)). It provides playback, rendering, daemon, and interactive
REPL commands, while this repository stays focused on the core library and
runtime.

```bash
fugue repl
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

The JavaScript browser player is published to npm as
[`@ilusiv/fyug-js`](https://www.npmjs.com/package/@ilusiv/fyug-js). This
repository owns the Rust engine and wasm exports that package consumes.

## YouTube Live Streaming

Native hosts can add a `youtube_sink` as a composable streaming tap. Audio is
passed through unchanged while a copy is sent to YouTube through `ffmpeg`.
Broadcast creation, scheduling, and privacy are managed in YouTube Studio.

Put the stream key from YouTube Live Control Room in the environment and keep it
out of invention JSON:

```sh
export YOUTUBE_STREAM_KEY='your-stream-key'
```

```json
{
  "id": "youtube",
  "type": "youtube_sink",
  "config": {
    "background_video": "./loop.mp4"
  }
}
```

The default profile is RTMPS, 1920x1080 at 30 fps, H.264 at 10 Mbps, AAC stereo
at 128 kbps, and a two-second GOP. Without `background_video`, the sink emits
black video so an audio-only invention still produces a valid A/V stream. The
sink has the same `audio`, `audio_left`, and `audio_right` inputs and outputs as
`rtmp_sink` and never lets streaming failure alter the pass-through signal.

Configuration overrides include `server_url`, `resolution` or `width`/`height`,
`fps`, `video_bitrate`, `audio_bitrate`, `video_encoder`, `audio_encoder`,
`gop_duration` (seconds), `ffmpeg_path`, `monitor`, `soft_clip`, `buffer_frames`, and
`video_queue_frames`. Set `tee_to_disk` to a local media path (for example,
`./broadcast.mkv`) to archive the encoded broadcast without a second encoding
pass. An archive write failure does not stop the network stream. Use
`stream_key_env` to name a different environment
variable. An explicit `stream_key_env` wins over `stream_key`; inline
`stream_key` is supported for convenience but is retained in invention/runtime
snapshots. In all modes, the resolved destination is necessarily passed to the
local `ffmpeg` process and may be visible to users who can inspect process
arguments.

`ffmpeg` must be installed and available in `PATH`. A real ingestion smoke test
is opt-in and should target an operator-created unlisted broadcast:

```sh
FUGUE_YOUTUBE_SINK_REAL_FFMPEG=1 \
  cargo test --lib real_youtube_ingestion_smoke -- --ignored
```

## MCP Server (AI-Driven Composition)

The `fugue-mcp` server is built and released by the Fugue product with the CLI
(see [Install](#install)). It exposes the runtime API as tools and talks to
`fugue serve` over the shared runtime RPC protocol.

## Design Docs

How the engine is meant to work, and why. Read the one that covers the code
you are changing before you change it.

- [Control model](docs/control-model.md): how every control change becomes a
  request the audio thread applies, and who wins when writes conflict.
- [Musical time](docs/musical-time.md): samples, conditions and clock beats,
  and where a score's notation ends and an invention's interpretation begins.
- [Module design](docs/module-design.md): behaviour lives in modules with
  ports, interfaces follow one naming convention, views of running audio
  show what is heard, modules see only their ports and controls, and when a
  behaviour is a native module or a development.
- [Wasm modules](docs/wasm-modules.md): the world a guest component
  implements, how it writes its outputs, and moving a guest to a new version.
- [Shared runtime](docs/shared-runtime.md): how clients find, edit and
  recover from lost replies to the one daemon that plays the music.
- [DSP provenance](docs/dsp-provenance.md): what may be ported from other
  projects, what must be written clean-room, and where tables and samples
  come from.
- [Atomic structural edits](docs/apply-edits.md) and
  [authored inspection](docs/authored-inspection.md): changing and reading the
  running invention's document.
