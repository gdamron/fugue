# Fugue <VERSION> for <TARGET>

Contents:

- `lib/libfugue.a`
- `lib/libfugue.dylib`

- `fugue.h`
- `LICENSE`
- `THIRD_PARTY_LICENSES.txt`

This package exposes the host-driven render API declared in `fugue.h`.
Render interleaved stereo `f32` frames from caller-owned buffers and set
runtime controls without embedding Rust in the host application.
