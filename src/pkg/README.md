# `fugue.pkg.json` manifest

Every Fugue extension — `module`, `development`, `invention`, `skill`,
`agent`, `sample-pack`, `policy` — ships with a `fugue.pkg.json` at its
root. This file is the shared manifest format that the `fugue install`
pipeline, the daemon, and the MCP server all read.

The Rust source of truth is [`PackageManifest`](manifest.rs). Validation
helpers live in [`validate.rs`](validate.rs); the entry points are
`parse_str`, `parse_path`, and `validate`.

## Schema

For the proposed shared catalog and import contract, including bundled starter
content, see [Musical content identity and resolution](CONTENT_IDENTITY.md).

| Field         | Type                      | Required | Notes                                                                         |
| ------------- | ------------------------- | -------- | ----------------------------------------------------------------------------- |
| `id`          | string                    | yes      | Reverse-DNS, ≥3 lowercase segments (`fugue.ns.name`).                         |
| `version`     | string                    | yes      | Semver.                                                                       |
| `kind`        | string enum               | yes      | One of `module`, `development`, `invention`, `skill`, `agent`, `sample-pack`, `policy`. |
| `license`     | string                    | yes      | SPDX identifier. Phase 1 does not look it up against the SPDX list.           |
| `authors`     | array of `{name, url?}`   | yes      | At least one entry; `name` must be non-empty.                                 |
| `description` | string                    | no       | One-line summary.                                                             |
| `homepage`    | string                    | no       | Project / docs URL.                                                           |
| `targets`     | array of string enum      | yes      | At least one of `external-agent`, `in-graph-agent`.                           |
| `requires`    | object                    | no       | Declared runtime requirements (see below).                                    |
| `deps`        | array of `id@req` strings | no       | Other Fugue packages this one depends on. Phase 1 shape-checks only.          |
| `policies`    | array of `id@req` strings | no       | Policy packages a `skill` or `agent` requires (see [Policies](#policies)).    |
| `examples`    | array of paths            | no       | Example invention or development documents in a `skill`; allowed only on skills. |
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
| `policy`      | `{ "policy": "<path>" }`      |

A `sample-pack`'s entry file (conventionally `samples.json`) has its own
kind-specific schema — licensing, attribution, sample rates, tags, and
slice points. See [SAMPLE_PACK.md](SAMPLE_PACK.md).

## Policies

A `policy` package is one Markdown file of shared rules or reference knowledge
with no triggers. Skills and agents list the policies they need in `policies`
instead of restating the rules:

```json
{ "kind": "skill", "policies": ["fugue.example.mixing-policy@^0.1.0"], "entry": { "skill": "SKILL.md" } }
```

- Only `skill` and `agent` packages may declare `policies`; on any other kind
  the field is a validation error, even when empty.
- Policies are flat: a policy package declares no `deps` and no `policies`.
- A policy is pinned, not live. The resolver walks `policies` alongside
  `deps` (`resolve_package`), chooses one version per policy id however many
  packages require it, and locks it like any dependency: one lockfile entry
  of kind `policy`, with its integrity hash, and an `id@version` edge in each
  requirer's `dependencies`. A requirement that resolves to a package of
  another kind fails resolution, naming the id and that kind.
- The entry must be a `.md` file inside the package that does not open with a
  YAML frontmatter block, which an agent host would discover as a skill.
  `check_policy_entry` checks this once the package is on disk.

## Examples

One per kind lives in [`examples/`](examples/):

- [`module.json`](examples/module.json)
- [`development.json`](examples/development.json)
- [`invention.json`](examples/invention.json)
- [`skill.json`](examples/skill.json)
- [`agent.json`](examples/agent.json)
- [`sample-pack.json`](examples/sample-pack.json)
- [`policy.json`](examples/policy.json)

These files are exercised by `tests/pkg_manifest_test.rs`, so they stay
in sync with the validator. The sample-pack entry file has its own
example, [`sample-pack.samples.json`](examples/sample-pack.samples.json),
exercised by `tests/pkg_sample_pack_test.rs`.

## Initial Scope

This module owns the manifest type and its validation only. Resolution,
installation, lockfile, capability enforcement at runtime, and signature
verification will be implemented in a future phase.
