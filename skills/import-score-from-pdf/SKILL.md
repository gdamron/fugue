---
name: import-score-from-pdf
description: Transcribe a score PDF into a validated fugue.score.v1 asset. Use when turning notated sheet music (PDF) into a Fugue score the sequencers can play. Pins poppler for a deterministic render + anchors, then transcribes page→system→measure with numeric self-checks.
---

# Import score from PDF

Turn a notated score **PDF** into a `fugue.score.v1` asset (a bank of cells in the
`{ note, gate, held, amplitude, grace }` step shape the sequencers consume).

This skill is **cross-platform**: the rendering/anchor step is a plain script
(`scripts/prep_pdf.sh` for macOS/Linux/WSL, `scripts/prep_pdf.ps1` for Windows).

We do **not** rasterize PDFs ourselves. Poppler is the canonical renderer the
toolchain already assumes; pinning it (version recorded in the manifest) is how
we get reproducibility. The _accuracy_ of a transcription is measured separately,
against a MusicXML/MIDI ground truth, by the import-accuracy harness — not by
pixel-identical rendering.

## When to use

- The user has a score as a PDF and wants it playable in Fugue.
- You need a deterministic, easily inspected view of a score's pages + embedded
  text.

## 1 — Preprocess (deterministic render + anchors)

Run the prep script for your platform. It ensures poppler is installed, renders
each page to a PNG at a pinned DPI, and extracts the PDF's text/metadata anchors.
Both scripts produce identical output.

```sh
# macOS / Linux / WSL / Git Bash
skills/import-score-from-pdf/scripts/prep_pdf.sh --install path/to/score.pdf out/
```

```powershell
# Windows (PowerShell)
skills\import-score-from-pdf\scripts\prep_pdf.ps1 -Install path\to\score.pdf out\
```

- Omit `--install` / `-Install` to have it only _print_ the platform install command
  if poppler is missing (installing mutates the system — see the script's preflight).
- `--dpi N` / `-Dpi N` overrides the pinned default (200). Keep it fixed for a piece.

**poppler is the one prerequisite.** If it isn't installed, the preflight prints
the right command for your platform:

| Platform      | Install                                                                                                                                                  |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS         | `brew install poppler`                                                                                                                                   |
| Debian/Ubuntu | `sudo apt-get install -y poppler-utils`                                                                                                                  |
| Fedora/RHEL   | `sudo dnf install -y poppler-utils`                                                                                                                      |
| openSUSE      | `sudo zypper install -y poppler-tools`                                                                                                                   |
| Arch          | `sudo pacman -S poppler`                                                                                                                                 |
| Alpine        | `sudo apk add poppler-utils`                                                                                                                             |
| Windows       | `winget install --id oschwartz10612.Poppler -e` (or `choco install poppler` / `scoop install poppler`; else install manually and put its `bin\` on PATH) |

Output in `out/`:

| File                          | Use                                                               |
| ----------------------------- | ----------------------------------------------------------------- |
| `page-1.png`, `page-2.png`, … | the pages to read, in order                                       |
| `info.txt`                    | `pdfinfo` metadata (Title, Author, Creator, page count/size)      |
| `text.txt`                    | `pdftotext -layout` embedded text (titles, tempo marks, etc.)     |
| `manifest.json`               | pinned poppler version + DPI + page list (the determinism record) |

## 2 — Fill metadata from anchors

Read `info.txt` and `text.txt` to populate the score's metadata:

- `title`, `composer` — from the title block / `info.txt` Author.
- `tempo` — from a marking like "♩ = 120" / "ca. 120".
- `time_signature` — `{ beats_per_measure, beat_unit }`.
- `key` — free-form (e.g. "Ab major").
- `base_note_hint` — a MIDI note the step offsets are relative to (pick a register
  anchor, e.g. 48/60).
- `rhythm_grid` — the smallest subdivision you will quantize to (e.g. "16th_note").

Treat anchors as _hints_: the page images are the source of truth for notes.

## 3 — Transcribe page → system → measure

Read the page PNGs in order. Working one system at a time, write the notes as
**cells** of steps. A cell is one array of steps; a through-composed piece can be
one cell per measure (or per system) — choose a consistent granularity and keep
cells aligned to the `rhythm_grid`.

Each step is one of:

- `null` — a rest.
- an integer — a note, as a semitone offset from `base_note_hint`.
- `{ "note": <int|null>, "gate": <0..1>, "held": <bool>, "amplitude": <0..1>, "grace": [<int>, …] }` —
  `held: true` continues the previous note without retriggering (ties / sustains);
  `note: null` is a rest; `gate` shortens the step's duration; `amplitude` is the
  dynamic level at this onset; `grace` is the step's grace-note chain.

**Grace notes**: the small slashed or small-head notes (acciaccaturas /
appoggiaturas) attach to the note step they decorate as a `grace` array of
semitone offsets from `base_note_hint`, in played order (the last grace
resolves into the principal). They are off the grid by definition: never give
a grace its own step or grid time, and never widen the `rhythm_grid` to fit
one. At most four per step; only note steps may carry `grace`. How a chain
sounds — timing, whether it steals from the previous beat, velocity — is the
sequencer's interpretation, not the score's.

**Dynamics**: capture dynamic marks (pp…fff) and hairpins as per-step `amplitude`
on note onsets, using the canonical mark → amplitude table in the score module
docs (`src/invention/score.rs`, "Dynamics") — each mark's conventional MIDI
velocity / 127 (p = 49/127 ≈ 0.386, mf = 80/127 ≈ 0.630, fff = 126/127 ≈ 0.992, …).
A mark holds until the next dynamic event; a hairpin interpolates linearly from
its start level to the next mark (one mark level up/down when no target follows).
Dynamics are part-level: a piano `p` governs both staves. Only note onsets carry
`amplitude` — held continuations and rests never do; notes before the first mark
carry none.

Assemble the `fugue.score.v1` document:

```json
{
  "schema": "fugue.score.v1",
  "title": "…", "composer": "…", "key": "…",
  "tempo": 120, "time_signature": { "beats_per_measure": 4, "beat_unit": 4 },
  "base_note_hint": 48, "rhythm_grid": "16th_note",
  "cells": [ [ 0, { "held": true }, 7, null ], [ … ] ]
}
```

## 4 — Validate against fugue.score.v1

Every candidate must conform to the schema. When the `fugue` CLI is installed,
run the authoritative validator directly:

```sh
fugue score validate draft-score.json   # prints OK / first error; exit 0 on pass
```

Treat a non-zero exit as a failed candidate: fix the reported problem and
re-validate before moving on.

If the CLI is not available, self-check the same shape the validator enforces
(`src/invention/score.rs::validate_score` in the `fugue` crate):

- top-level is an object; if `schema` is present it must be `"fugue.score.v1"`;
- `cells` is present and non-empty, and every cell is non-empty;
- every step is `null`, an integer in `-128..=127`, or an object whose `note` is
  an integer/null and `gate` and `amplitude` are in `0..1`; a `held` step carries
  only `{ "held": true }`; `grace` is a non-empty array of at most 4 integers in
  `-128..=127`, only on steps with an integer `note`;
- `base_note_hint` is `0..=127`; `tempo` > 0; time-signature fields are positive.

## 5 — Self-verify (numeric guards) and iterate

Before declaring done, cross-check the transcription against what the page shows:

- **Measure count** — number of measures transcribed matches the score.
- **Beats per measure** — each measure's summed durations match the time signature.
- **Note density** — steps-per-system is plausible (no silently empty or overfull
  cells).
- **Dynamics coverage** — every dynamic mark and hairpin on the page appears in
  the amplitude sequence (spot-check section boundaries and climaxes).

Fix mismatches and repeat from step 3 until the guards pass.

## Output

Write the validated document to where the caller wants it — for an example piece,
`examples/<piece>/score.json` (mirroring `examples/in_c/score.json`).
