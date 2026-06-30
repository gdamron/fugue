# import-score-from-pdf

A first-party Fugue skill that turns a notated score **PDF** into a validated
[`fugue.score.v1`](../../src/invention/score.rs) asset.

## Why it's shaped this way

- **Agent-agnostic.** The skill is a neutral bundle, not a Claude-only file. The
  heavy lifting (`scripts/prep_pdf.sh`) is plain shell + poppler that any
  harness — Claude Code, Codex, the in-graph Agent module, or a human — can run.
  `SKILL.md` is portable content; `fugue.skill.json` declares the bundle.
- **No native rasterizer.** We don't render PDFs in Rust. Poppler is the renderer
  the surrounding toolchain already assumes; we pin it (version recorded in
  `manifest.json`) for reproducibility. Rationale:
  `.harness/decisions/2026-06-30-judgment.md`.
- **Rigor lives downstream.** Pixel-identical rendering is a weak proxy for
  accuracy; transcription quality is measured by the import-accuracy harness
  (FUG-174) against a MusicXML/MIDI ground truth.

## Layout

```
import-score-from-pdf/
  SKILL.md            portable instructions (front-matter + body)
  fugue.skill.json    manifest stub (id, version, targets, requires, produces)
  scripts/prep_pdf.sh poppler preflight + deterministic render + anchors + manifest
  README.md           this file
```

## Quick start

```sh
scripts/prep_pdf.sh --install path/to/score.pdf out/
# out/page-*.png  out/info.txt  out/text.txt  out/manifest.json
```

Then follow `SKILL.md` to read the pages + anchors and transcribe to
`fugue.score.v1`.

## Forward-compatibility

`fugue.skill.json` follows the bundle shape proposed by the Fugue Skills & Agent
Definitions work (FUG-119/120) but does **not** depend on its (unbuilt) manifest
validator or dual-life installer. When those land, this bundle should install
cleanly into both `~/.claude/skills/` and `~/.fugue/skills/`.
