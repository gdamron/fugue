# `sample-pack` entry (`samples.json`)

A `sample-pack` package's `fugue.pkg.json` (the shared base manifest, see
[README.md](README.md)) names its entry file via `entry.samples` —
conventionally `samples.json` at the package root. The entry file is the
kind-specific manifest defined here: it carries everything a composer or
module needs to know about the audio content itself — licensing,
attribution, sample rates, descriptive tags, and slice points.

The Rust source of truth is [`SamplePackManifest`](sample_pack.rs);
the parse/validate entry points are `pkg::sample_pack::{parse_str,
parse_path, validate}`.

## Schema

| Field         | Type                    | Required | Notes                                                        |
| ------------- | ----------------------- | -------- | ------------------------------------------------------------ |
| `license`     | string                  | yes      | Pack-wide SPDX identifier. Files may override.               |
| `attribution` | array of `{name, url?}` | no       | Creators of the audio content (may differ from the package `authors`, who assembled the pack). |
| `sample_rate` | array of integer        | yes      | Rates present in the pack, in Hz. Every declared rate must be used by at least one file. |
| `tags`        | object of string→string | no       | Free-form (`genre`, `instrument`, `bpm`, `key`, ...). Values are strings even when numeric (`"bpm": "120"`). |
| `files`       | array of file objects   | yes      | Explicit listing of every audio file (see below). At least one entry. |

### `files[]`

Each file entry lists one audio file and may override the pack-wide
defaults:

| Field         | Type                    | Required | Notes                                                        |
| ------------- | ----------------------- | -------- | ------------------------------------------------------------ |
| `path`        | string                  | yes      | Relative path inside the package (`samples/kick.wav`). No `.`/`..` segments, no absolute paths, unique across the listing. |
| `license`     | string                  | no       | SPDX override for this file.                                 |
| `attribution` | array of `{name, url?}` | no       | Attribution override for this file.                          |
| `sample_rate` | integer                 | no*      | This file's rate in Hz. Must be one of the pack's declared rates. *Required when the pack declares more than one rate; when it declares exactly one, files inherit it. |
| `tags`        | object of string→string | no       | Merged over the pack-wide tags.                              |
| `slices`      | array of slice objects  | no       | Slice points into this file (see below).                     |

Effective per-file values resolve override-first: `license_for`,
`attribution_for`, and `sample_rate_for` on `SamplePackManifest` implement
this fallback so consumers never re-derive it.

### `slices[]`

Slice points address regions of a file in frames. They are consumed by
slicing modules (`sample_slicer`) to trigger regions individually —
breakbeat re-sequencing, chops, one-shots inside a stem.

| Field          | Type    | Required | Notes                                             |
| -------------- | ------- | -------- | ------------------------------------------------- |
| `start_frames` | integer | yes      | First frame of the slice (inclusive).             |
| `end_frames`   | integer | yes      | End frame (exclusive). Must exceed `start_frames`. |
| `name`         | string  | no       | For addressing the slice by name (`"kick"`). Unique within the file when present; unnamed slices are addressed by index. |

Slices may overlap — layered or round-robin chops are legitimate.
Validation does not check `end_frames` against the actual audio length
(the manifest is validated without decoding audio); loaders clamp or
reject out-of-range slices at load time.

## Example

```json
{
  "license": "CC0-1.0",
  "attribution": [{ "name": "Example Sampler", "url": "https://example.com" }],
  "sample_rate": [44100, 48000],
  "tags": { "genre": "electronic", "instrument": "drums", "bpm": "120" },
  "files": [
    {
      "path": "samples/kick.wav",
      "sample_rate": 48000,
      "tags": { "instrument": "kick drum" }
    },
    {
      "path": "samples/break.wav",
      "sample_rate": 44100,
      "license": "CC-BY-4.0",
      "attribution": [{ "name": "Break Creator" }],
      "slices": [
        { "start_frames": 0, "end_frames": 22050, "name": "kick" },
        { "start_frames": 22050, "end_frames": 44100, "name": "snare" }
      ]
    }
  ]
}
```

The full example lives at
[`examples/sample-pack.samples.json`](examples/sample-pack.samples.json)
and is exercised by `tests/pkg_sample_pack_test.rs`, so it stays in sync
with the validator. The first reference pack is `fugue.demo.808-kit` in
the `fugue-packs` repository.

## Validation summary

`validate` rejects, in order of first failure:

- empty pack or per-file `license`
- attribution entries with empty names
- an empty `sample_rate` list, a rate of `0`, a per-file rate not in the
  declared list, a file with no resolvable rate (pack declares several,
  file picks none), or a declared rate no file uses
- an empty `files` list, non-relative or duplicate paths
- slices whose `end_frames` ≤ `start_frames`, empty slice names, or
  duplicate slice names within one file
