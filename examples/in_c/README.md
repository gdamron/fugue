# In C — Terry Riley (1964)

A generative performance of Terry Riley's *In C* assembled from Fugue's
general-purpose modules.

> "Patterns are to be played consecutively with each performer having the
> freedom to determine how many times he or she will repeat each pattern
> before moving on to the next."  — Terry Riley, *In C* performing instructions

This invention realizes those instructions in software: a conductor observes
all 13 voices and decides when each one should move through the 53 melodic
cells, while keeping the ensemble loosely aligned and musical.

## Run it

```
cargo run --release --example examples -- --example in_c.json
```

Use `--release`. With 13 voices, a 20-channel mixer, reverb, scripting, and
agent orchestration, the dev profile leaves enough margin missing to drop
frames on most laptops. Press Enter to stop. The performance continues
indefinitely; pattern 53 is treated as a gathering point before the cycle can
begin again.

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
- **`conductor`** — an `agent` module loaded with `conductor.md`. It wakes
  on the clock's whole-note gate (`gate_d4`), reads every voice's
  `current_cell` and `loop_count`, and writes `advance` decisions back to
  the `mel_<i>` sequencers. It also writes mixer channel levels and reverb
  wet to shape the macro arc. Configure the backend with
  `conductor.config.backend`; the default is `local:auto`.
- **`conductor_fallback`** — a `code` module that keeps sequencer `steps`
  aligned with the active cell length. If the conductor is disabled or has
  not completed a request recently, it applies conservative deterministic
  conducting rules so the example still progresses without LLM credentials.
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
| `conductor.config.backend` | LLM/local harness used for primary conducting. |
| `conductor.config.cooldown_ms` | Minimum spacing between conductor requests. Higher = broader, slower decisions. |
| `conductor_fallback.config.min_loops_before_advance` | Minimum loops a voice spends on a cell before fallback considers advancing. Higher = slower, more meditative. |
| `conductor_fallback.config.max_cells_ahead_of_slowest` | How far fallback lets a voice run ahead of the slowest peer. Lower = tighter ensemble. |
| `conductor_fallback.config.advance_probability` | Per-tick fallback probability of advancing once min-loops is satisfied and the voice is within range. |
| `mixer.config.levels` / `pans` | Per-channel mix. Channel 1 is the pulse; 2–14 are the voices. |
| Voice count | Add or remove voice pairs (`mel_<i>` + `voice_<i>`) and update the conductor context/apply mappings, fallback `sequencer_ids`, and mixer channel mapping. |

## Score data

`score.json` is a transcription of the 53 cells against a 32nd-note grid.
See `score-notes.md` for encoding notes and the audit checklist. Treat it
as a working asset that should be cross-referenced against the project PDF
before quoting it as publication-quality data.

## Credit

*In C* was composed by Terry Riley in 1964. This example is a software
realization that follows Riley's original performing instructions; the
underlying composition remains his.
