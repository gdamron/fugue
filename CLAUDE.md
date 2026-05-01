# Agent Info

In general, the codebase is the source of truth for architecture and design
decision. This document provides some high-level guidance on how to develop and
maintain the codebase.

## Performance is the Top Priority

Ears are nature's alarm system. Unintentional clicks, pops, and glitches are
jarring and can cause listeners to stop listening. With this in mind, ensuring
clean audio -- that is, audio that sounds as the user intended -- is the top
priority.

- Keep audio-thread code allocation-free (no `Vec::new()`, etc.)
- Keep audio-thread code lock-free. Module `process()` runs at sample rate
  (tens of thousands of times per second per module), so a `Mutex` shared
  with control/script threads will starve the audio callback and produce
  dropouts. Share state with control/script threads via `std::sync::atomic`
  primitives (or `crate::atomic::AtomicF32`) on the hot path. If a `Mutex`
  is unavoidable for a non-trivial value (e.g. a `Vec`), gate it behind an
  atomic version counter so the audio thread only locks when it actually
  changed. See `src/modules/cell_sequencer/controls.rs` and
  `src/modules/mixer/controls.rs` for the pattern.
- Use pre-allocated buffers for DSP
- Prefer `f32` math for audio-rate signals (SIMD-friendly)
- Reset phase accumulators using `%=` to prevent drift
- Use `Send` marker for thread-safe types
- Cache results of expensive math (`powf`, `exp`, `log`, `sin`, `cos`,
  `tan`) instead of recomputing per sample. Prefer a precomputed lookup
  table or recompute-on-change for any value that varies less often than
  the sample rate. See `src/music/note.rs`'s `MIDI_FREQUENCIES` table for
  the pattern.

## This is a Music Creation Tool

While this project takes heavy inspiration from modular synthesis, it is not a
modular synth in the traditional sense. It is a tool for creating music, and all
design decisions should be made with that in mind. Think like a composer first,
and an engineer second.

## General Instructions

- When you get stuck, stop and ask for help. This is a collaborative project.
- In general, the current working branch is named after the Linear issue being
  developed. Or, the current session began with a request to work on a specific
  issue. Linear should be available over MCP, so you can add comments to
  report progress, ask questions, or request help.
- A human developer is responsible for your output. Do not claim co-authorship
  or full authorship of any code you write or in commit messages. You may create
  branches, commit code, push to the remote repository, and open pull requests,
  but you may not merge pull requests into main.
