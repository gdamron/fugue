# import-score-from-pdf

A first-party Fugue skill that turns a notated score **PDF** into a validated
[`fugue.score.v1`](../../src/invention/score.rs) asset.

## Why it's shaped this way

- **Agent-agnostic & cross-platform.** The skill is a neutral bundle. The heavy
  lifting is plain poppler orchestration any harness can run: POSIX `scripts/prep_pdf.sh`
  (macOS/Linux/WSL/Git Bash) and `scripts/prep_pdf.ps1` (native Windows
  PowerShell), with identical output. `SKILL.md` is portable content; `fugue.skill.json`
  declares the bundle.
- **No native rasterizer, and rigor lives downstream.** See
  [Why no PDF rasterizer in the engine](#why-no-pdf-rasterizer-in-the-engine).

## Why no PDF rasterizer in the engine

The engine never opens a PDF. A score PDF is input for the agent doing the
transcription, so rendering it is this skill's job, not the runtime's.

- **No native dependency in the audio runtime.** A PDF renderer such as pdfium
  or mupdf is a large native library. Linking one into the engine or the CLI
  would add build and platform risk to the program that plays music, for a
  feature that never runs while it plays.
- **One renderer, not two.** Agent harnesses already render PDF pages with
  poppler, and so do the tools an agent reads files with. A second renderer
  in Rust would draw some pages differently from what the agent sees.
- **Reproducible through a recorded tool.** `scripts/prep_pdf.sh` and
  `scripts/prep_pdf.ps1` call poppler (`pdftocairo`, `pdftotext`, `pdfinfo`)
  at a fixed DPI (200 by default). They record the poppler version and DPI in
  `manifest.json`, so a run can be repeated and compared.
- **Accuracy is measured on the music, not the pixels.** Identical rendering
  says little about whether the notes are right. A transcription is scored
  against a reference converted from MusicXML (`invention::score::import`):
  `invention::score::compare` matches notes by onset and pitch and reports
  precision, recall, F1 and duration accuracy. The product CLI exposes it as
  `score compare`, with `--require-exact` and `--min-f1` for use as a gate.

## Quick start

```sh
# macOS / Linux / WSL / Git Bash
scripts/prep_pdf.sh --install path/to/score.pdf out/
# out/page-*.png  out/info.txt  out/text.txt  out/manifest.json
```

```powershell
# Windows
scripts\prep_pdf.ps1 -Install path\to\score.pdf out\
```

Then follow `SKILL.md` to read the pages + anchors and transcribe to
`fugue.score.v1`.
