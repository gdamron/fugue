# `fugue.pkg.json` manifest

Every Fugue extension — `module`, `development`, `invention`, `skill`,
`agent`, `sample-pack` — ships with a `fugue.pkg.json` at its
root. This file is the shared manifest format that the `fugue install`
pipeline, the daemon, and the MCP server all read.

The Rust source of truth is [`PackageManifest`](manifest.rs). Validation
helpers live in [`validate.rs`](validate.rs); the entry points are
`parse_str`, `parse_path`, and `validate`.

## Schema

| Field         | Type                      | Required | Notes                                                                         |
| ------------- | ------------------------- | -------- | ----------------------------------------------------------------------------- |
| `id`          | string                    | yes      | Reverse-DNS, ≥3 lowercase segments (`fugue.ns.name`).                         |
| `version`     | string                    | yes      | Semver.                                                                       |
| `kind`        | string enum               | yes      | One of `module`, `development`, `invention`, `skill`, `agent`, `sample-pack`. |
| `license`     | string                    | yes      | SPDX identifier. Phase 1 does not look it up against the SPDX list.           |
| `authors`     | array of `{name, url?}`   | yes      | At least one entry; `name` must be non-empty.                                 |
| `description` | string                    | no       | One-line summary.                                                             |
| `homepage`    | string                    | no       | Project / docs URL.                                                           |
| `targets`     | array of string enum      | yes      | At least one of `external-agent`, `in-graph-agent`.                           |
| `requires`    | object                    | no       | Declared runtime requirements (see below).                                    |
| `deps`        | array of `id@req` strings | no       | Other Fugue packages this one depends on. Phase 1 shape-checks only.          |
| `entry`       | object                    | yes      | Kind-specific entry point (see below).                                        |
| `signing`     | `{key, signature}`        | no       | Reserved for Phase 2; accepted but not enforced.                              |

### `requires`

```json
{
  "mcp-tools": ["fugue"],
  "capabilities": ["random", "time", "fs:read:samples/", "net:example.com"]
}
```

Recognized capability prefixes: `random`, `time`, `fs:read:<scope>`,
`fs:write:<scope>`, `net:<host>`. Unknown prefixes are a validation
error so typos surface at install time.

### `entry`

The `entry` block is keyed by the kind's expected entry field. Validation
rejects manifests whose `entry` shape doesn't match `kind`.

| `kind`        | `entry` shape                 |
| ------------- | ----------------------------- |
| `module`      | `{ "wasm": "<path>" }`        |
| `development` | `{ "development": "<path>" }` |
| `invention`   | `{ "invention": "<path>" }`   |
| `skill`       | `{ "skill": "<path>" }`       |
| `agent`       | `{ "definition": "<path>" }`  |
| `sample-pack` | `{ "samples": "<path>" }`     |

A `sample-pack`'s entry file (conventionally `samples.json`) has its own
kind-specific schema — licensing, attribution, sample rates, tags, and
slice points. See [SAMPLE_PACK.md](SAMPLE_PACK.md).

## Examples

One per kind lives in [`examples/`](examples/):

- [`module.json`](examples/module.json)
- [`development.json`](examples/development.json)
- [`invention.json`](examples/invention.json)
- [`skill.json`](examples/skill.json)
- [`agent.json`](examples/agent.json)
- [`sample-pack.json`](examples/sample-pack.json)

These files are exercised by `tests/pkg_manifest_test.rs`, so they stay
in sync with the validator. The sample-pack entry file has its own
example, [`sample-pack.samples.json`](examples/sample-pack.samples.json),
exercised by `tests/pkg_sample_pack_test.rs`.

## Initial Scope

This module owns the manifest type and its validation only. Resolution,
installation, lockfile, capability enforcement at runtime, and signature
verification will be implemented in a future phase.
