---
name: import-score-from-pdf
description: Transcribe a score PDF into a validated fugue.score.v1 asset. Use when turning notated sheet music (PDF) into a Fugue score the sequencers can play. Pins poppler for a deterministic render + anchors, then transcribes page→system→measure with numeric self-checks.
---

# Import score from PDF

Turn a notated score **PDF** into a `fugue.score.v1` asset (a bank of cells in the
`{ note, gate, held }` step shape the sequencers consume).

This skill is **agent-agnostic**: the rendering/anchor step is a plain script
(`scripts/prep_pdf.sh`) any harness can run; the transcription is performed by
whatever agent is reading this. Nothing here is specific to one assistant.

We do **not** rasterize PDFs ourselves. Poppler is the canonical renderer the
toolchain already assumes; pinning it (version recorded in the manifest) is how we
get reproducibility. The *accuracy* of a transcription is measured separately,
against a MusicXML/MIDI ground truth, by the import-accuracy harness (FUG-174) —
not by pixel-identical rendering.

## When to use

- The user has a score as a PDF and wants it playable in Fugue.
- You need a deterministic, inspectable view of a score's pages + embedded text.

## 1 — Preprocess (deterministic render + anchors)

Run the prep script. It ensures poppler is installed, renders each page to a PNG at
a pinned DPI, and extracts the PDF's text/metadata anchors:

```sh
skills/import-score-from-pdf/scripts/prep_pdf.sh --install path/to/score.pdf out/
```

- Omit `--install` to have it only *print* the platform install command if poppler
  is missing (installing mutates the system — see the script's preflight).
- `--dpi N` overrides the pinned default (200). Keep it fixed for a given piece.

Output in `out/`:

| File | Use |
|------|-----|
| `page-1.png`, `page-2.png`, … | the pages to read, in order |
| `info.txt` | `pdfinfo` metadata (Title, Author, Creator, page count/size) |
| `text.txt` | `pdftotext -layout` embedded text (titles, tempo marks, etc.) |
| `manifest.json` | pinned poppler version + DPI + page list (the determinism record) |

## 2 — Fill metadata from anchors

Read `info.txt` and `text.txt` to populate the score's metadata:

- `title`, `composer` — from the title block / `info.txt` Author.
- `tempo` — from a marking like "♩ = 120" / "ca. 120".
- `time_signature` — `{ beats_per_measure, beat_unit }`.
- `key` — free-form (e.g. "Ab major").
- `base_note_hint` — a MIDI note the step offsets are relative to (pick a register
  anchor, e.g. 48/60).
- `rhythm_grid` — the smallest subdivision you will quantize to (e.g. "16th_note").

Treat anchors as *hints*: the page images are the source of truth for notes.

## 3 — Transcribe page → system → measure

Read the page PNGs in order. Working one system at a time, write the notes as
**cells** of steps. A cell is one array of steps; a through-composed piece can be
one cell per measure (or per system) — choose a consistent granularity and keep
cells aligned to the `rhythm_grid`.

Each step is one of:

- `null` — a rest.
- an integer — a note, as a semitone offset from `base_note_hint`.
- `{ "note": <int|null>, "gate": <0..1>, "held": <bool> }` — `held: true` continues
  the previous note without retriggering (ties / sustains); `note: null` is a rest;
  `gate` shortens the step's duration.

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

Every candidate must conform to the schema (authoritative validator:
`src/invention/score.rs::validate_score` in the `fugue` crate). Self-check the shape:

- top-level is an object; if `schema` is present it must be `"fugue.score.v1"`;
- `cells` is present and non-empty, and every cell is non-empty;
- every step is `null`, an integer in `-128..=127`, or an object whose `note` is an
  integer/null and `gate` is in `0..1`; a `held` step carries only `{ "held": true }`;
- `base_note_hint` is `0..=127`; `tempo` > 0; time-signature fields are positive.

## 5 — Self-verify (numeric guards) and iterate

Before declaring done, cross-check the transcription against what the page shows:

- **Measure count** — number of measures transcribed matches the score.
- **Beats per measure** — each measure's summed durations match the time signature.
- **Note density** — steps-per-system is plausible (no silently empty or overfull
  cells).

Fix mismatches and repeat from step 3 until the guards pass. These are cheap
structural checks; true note-level accuracy is scored later by FUG-174 against a
MusicXML/MIDI ground truth.

## Output

Write the validated document to where the caller wants it — for an example piece,
`examples/<piece>/score.json` (mirroring `examples/in_c/score.json`).
