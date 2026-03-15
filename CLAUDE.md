# Agent Info

In general, the codebase is the source of truth for architecture and design 
decision. This document provides some high-level guidance on how to develop and 
maintain the codebase.

### Performance is the Top Priority

Ears are nature's alarm system. Unintentional clicks, pops, and glitches are 
jarring and can cause listeners to stop listening. With this in mind, ensuring
clean audio -- that is, audio that sounds as the user intended -- is the top 
priority.

- Keep audio-thread code allocation-free (no `Vec::new()`, etc.)
- Use pre-allocated buffers for DSP
- Prefer `f32` math for audio-rate signals (SIMD-friendly)
- Reset phase accumulators using `%=` to prevent drift
- Use `Send` marker for thread-safe types

### This is a Music Creation Tool

While this project takes heavy inspiration from modular synthesis, it is not a 
modular synth in the traditional sense. It is a tool for creating music, and all 
design decisions should be made with that in mind. Think like a composer first,
and an engineer second.

### General Instructions

- When you get stuck, stop and ask for help. This is a collaborative project.
- In general, the current working branch is named after the Linear issue being 
  developed. Or, the current session began with a request to work on a specific
  issue. Linear should be available over MCP, so you can add comments to 
  report progress, ask questions, or request help.
- A human developer is responsible for your output. Do not claim partial or 
  full authorship of any code you write. You may create branches, commit code,
  push to the remote repository, and open pull requests, but you may not 
  merge pull requests into main.
