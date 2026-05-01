# In C — Terry Riley (1964)

A generative performance of Terry Riley's *In C* assembled from Fugue's
general-purpose modules.

> "Patterns are to be played consecutively with each performer having the
> freedom to determine how many times he or she will repeat each pattern
> before moving on to the next."  — Terry Riley, *In C* performing instructions

This invention realizes those instructions in software: each voice owns its
own progression through the 53 melodic cells, deciding how many loops to
spend on each cell before advancing, while keeping loose alignment with its
peers.

## Run it

```
cargo run --release --example examples -- --example in_c.json
```

Use `--release`. With 13 voices, a 20-channel mixer, and reverb, the dev
profile leaves enough margin missing to drop frames on most laptops. Press
Enter to stop. The performance continues indefinitely — voices wrap back
to cell 1 after reaching cell 53 (configurable, see below).

## Structure

- **`clock`** — master clock at `bpm = 240` so that one beat at the clock's
  rate is one 8th note in *In C*'s notional 120 BPM tempo, and `gate_x4` is
  the 32nd-note grid the score is encoded against.
- **`pulse`** — the famous In C pulse: a constant C5 8th-note pluck
  (`pluck_voice` development), driven directly off the clock's beat gate.
- **13 voices**, each composed of:
  - `mel_<i>` — a `cell_sequencer` loaded with the 53 cells from
    `score.json` (referenced via the shared `score` asset).
  - `voice_<i>` — a development instance from `examples/developments/`
    (piano, marimba, vibraphone, pluck, or pad — distributed across voices
    for timbral variety).
  - `prog_<i>` — a `code` module running `voice_progression.js`. It reads
    each voice's `loop_count` and peer voices' `current_cell`, and pulses
    `advance` on its own sequencer when the musical conditions are met. It
    also keeps the sequencer's `steps` aligned with the active cell's
    length (cells in the score range from 4 to 256 32nd-note steps).
- **`conductor`** — an `agent` module loaded with `conductor.md`. Disabled
  by default so the example runs without LLM credentials. When enabled it
  wakes on the clock's whole-note gate (`gate_d4`) and writes to mixer
  channel levels and reverb wet to shape the macro arc. To enable, set
  `enabled: true` on the `conductor` module and configure a backend.
- **`mixer`** — 20 channels: channel 1 is the pulse, channels 2–14 are the
  13 voices spread across the stereo field, channels 15–20 are headroom for
  the conductor or future per-voice routing.
- **`reverb`** — a single shared room shared by every voice including the
  pulse.

## Tunable parameters

Edit `examples/in_c.json` directly:

| Where | What it controls |
| --- | --- |
| `clock.config.bpm` | Tempo. `240` corresponds to 120 BPM at the 8th-note pulse; halve for half-time, double for double-time. |
| `prog_<i>.config.min_loops_before_advance` | Minimum loops a voice spends on a cell before it considers advancing. Higher = slower, more meditative. |
| `prog_<i>.config.max_cells_ahead_of_slowest` | How far a voice will run ahead of the slowest peer. Lower = tighter ensemble. |
| `prog_<i>.config.advance_probability` | Per-tick probability of advancing once min-loops is satisfied and the voice is within range. Lower = longer dwell time per cell. |
| `prog_<i>.config.last_cell_behavior` | `"loop"` (default here) wraps cell 53 → cell 1 so the performance never terminates. `"hold"` parks the voice on cell 53. |
| `mixer.config.levels` / `pans` | Per-channel mix. Channel 1 is the pulse; 2–14 are the voices. |
| Voice count | Add or remove voice triples (`mel_<i>` + `voice_<i>` + `prog_<i>`) and update the corresponding `peer_voice_ids` lists and mixer channel mapping. |

## Score data

`score.json` is a transcription of the 53 cells against a 32nd-note grid.
See `score-notes.md` for encoding notes and the audit checklist. Treat it
as a working asset that should be cross-referenced against the project PDF
before quoting it as publication-quality data.

## Credit

*In C* was composed by Terry Riley in 1964. This example is a software
realization that follows Riley's original performing instructions; the
underlying composition remains his.
