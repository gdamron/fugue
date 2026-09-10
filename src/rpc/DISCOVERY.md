# Module discovery

Discovery separates registered type names, type defaults, supplied configuration,
and existing instances. It does not search available-but-unregistered content.

## Requests

`describe_module_types` defaults to a terse index. Optional `types` selects exact
registered names; `detail: "full"` requests default-config metadata:

```json
{"schema_version":1,"kind":"command","command":"describe_module_types","types":["divisi","mixer"],"detail":"full"}
```

`describe_module` accepts exactly one of `type` or `module_id`:

```json
{"schema_version":1,"kind":"command","command":"describe_module","type":"mixer","config":{"channels":8}}
```

```json
{"schema_version":1,"kind":"command","command":"describe_module","module_id":"mix"}
```

Omitted or null config means type defaults. Supplied config uses the same factory
validation, defaults and clamping as live module creation (`add_module`/`swap_module`).
For example, mixer currently defaults a wrong-typed channel count while divisi
rejects an out-of-range voice count. Inspection adds no stricter module validation.
Invention-document `$asset` expansion is not part of these live factory calls.

## Responses and provenance

RPC responses nest the discovery payload under `discovery` (`kind: "module_types"`)
or `description` (`kind: "module_description"`). MCP returns that payload as one
compact JSON text block. The nested discovery `schema_version: 1` versions this
contract independently of the RPC envelope's schema version. This replaces the
previous unversioned full catalog shape; update discovery consumers together.

The terse payload is `{schema_version, registry_scope, types}`. Full catalogs also
include `metadata_source: "type_defaults"`, `sample_rate`, and `details`. Each
detail has `status: "available"` with `type_name`, ports, controls and `is_sink`, or
`status: "unavailable"` with a type name and explicit error. An unavailable entry
has no fabricated empty ports or controls. A type requiring config may appear in
the index even though its defaults cannot be inspected.

Single-module responses include `metadata_source` (`type_defaults`,
`supplied_config`, or `running_instance`), sample rate, type name, ports, controls,
`is_sink`, and `control_values`. Live responses also include `module_id`.
Control metadata's `default` remains the declared default; `control_values` carries
the inspected initial or current values. Unsupported value reads are omitted.
An empty controls array is valid only after successful inspection of a module
that exposes no controls.

Types are unique and sorted lexicographically. Full entries follow the same
order; controls are sorted by key. Ports retain their declared order because
port indices have meaning.

## Registry lifetime

While an invention is loaded, discovery uses its current registry, including
registered developments (`registry_scope: "running"`). While idle, only built-ins
are available (`"builtins"`); instance requests fail. Built-ins depend on platform
and compiled features.

Results are uncached. Successful reloads immediately expose updated definitions
and remove unregistered types; failed reloads leave the last good registry intact.
Instance inspection reads captured instance ports and live control surfaces
without rebuilding. Swaps, removals and unloads are reflected on the next call.
Clients must requery after those operations and after reconnecting to a daemon.

## Limits and errors

- A `types` filter accepts 1–64 names. Duplicate names are collapsed; unknown
  names fail the entire query. Full discovery inspects at most 64 types per call.
- Selector names/IDs accept 1–256 UTF-8 bytes. Config accepts at most 16,384
  serialized UTF-8 JSON bytes.
- A successful discovery payload is at most 65,536 serialized UTF-8 JSON bytes.
  The MCP result envelope is additionally limited to 131,072 bytes; JSON-RPC
  framing is outside that limit. Oversized responses are errors, never truncated
  successes. Narrow the type filter or inspect a single module.
- RPC error codes are `invalid_request`, `unknown_module_type`, `unknown_module`,
  `module_build_failed`, and `response_too_large`. MCP preserves the RPC error
  name and message in its error response. Full catalogs embed per-type inspection
  failures; individual inspection calls return an error directly.

Inspection runs on the control thread and never starts audio, recording, or
streaming. Sink inspection validates config without opening destinations or
launching ffmpeg, so it does not guarantee external output readiness. Sample
assets may be read. Wasm construction may execute guest code and is explicitly
unavailable for type/config inspection; inspect an existing instance instead.
Custom factories with build-time effects must override `build_for_inspection`
with a metadata-only implementation or an error. Nested development builds
preserve inspection mode.
