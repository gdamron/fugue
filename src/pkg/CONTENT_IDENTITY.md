# Musical content identity and resolution

Status: proposed contract for implementation. This document specifies new catalog
and reference behavior; its examples are not claims that current loaders accept
the new fields. Tracking: FUG-267; consumers: FUG-251 (developments), FUG-252
(packages/install), FUG-253 (playable inventions). This decision owns schemas and
examples only, not tools, an index, content migration, or release packaging.

## Decision

Use ordinary Fugue packages for distributed musical content. `fugue-packs` is
the canonical authoring home. Bundling is a delivery method, not a different
package identity. Catalogs describe content available to the daemon; they do not
register types in the running invention. Importing a development explicitly
binds its reference to an invention-local module type name.

Ship the five instruments currently in `examples/developments`: piano, marimba,
vibraphone, pluck, and pad. Ship BWV 772 as the starter invention, using the
MusicXML and score fixture currently in `fugue-cli/tests/fixtures/bwv772` as its
source. Do not bundle In C, the demo pulse packages, or the remaining repository
examples by default. `voice_library_trio.json` is an invention, not an instrument.

Draft arrangement assumption: use the bundled piano for both Bach parts, with
independent voices and explicit routing. Other instruments remain available to
substitute. This musical choice does not affect the identity contract.

Each instrument is one `development` package because the existing manifest has
one kind-specific entry. The Bach package is an `invention` with its score asset
and declared instrument dependencies. “Starter pack” means this release selection,
not a new manifest kind or a second registry.

Proposed initial coordinates (reserved by this decision, not published today):

| ID | Version | Kind | Content source |
| --- | --- | --- | --- |
| `fugue.instruments.piano` | `0.1.0` | development | `examples/developments/piano.json` |
| `fugue.instruments.marimba` | `0.1.0` | development | `examples/developments/marimba.json` |
| `fugue.instruments.vibraphone` | `0.1.0` | development | `examples/developments/vibraphone.json` |
| `fugue.instruments.pluck` | `0.1.0` | development | `examples/developments/pluck.json` |
| `fugue.instruments.pad` | `0.1.0` | development | `examples/developments/pad.json` |
| `fugue.starter.bwv772` | `0.1.0` | invention | Bach score plus a new playable arrangement |

The Bach fixture is score data, not yet a runnable invention. Arrangement and
listening verification are required before advertising it as playable. Preserve
the fixture's recorded CC-BY-SA-3.0 license and Jeff Covey/Mutopia attribution in
the score distribution. Keep the score and performance interpretation separate.

## Existing foundations and required extensions

- [PackageManifest](manifest.rs) supplies package ID, SemVer, kind, summary,
  dependencies, license, authors, and entry. Do not create competing metadata.
- [Version selection](resolve.rs) and [package asset lookup](audio_asset.rs)
  provide installed-version/lock behavior and the package cache root.
- [DevelopmentSpec](../invention/format.rs) currently accepts `path` or
  `definition`. The contract adds a third mutually exclusive choice, `ref`.
- [Development loading](../invention/builder.rs) preserves each loaded file's
  source path for relative dependencies. Reference loading must do the same.
- [PLAYABLE_EXAMPLES](../example_catalog.rs) currently discovers source-tree
  files relative to the working directory. Reuse its curated-example mechanism
  for the selected packaged starter content; do not expose the whole old list
  in releases or use checkout paths as load references.

`fugue-packs` already validates manifests and entry layouts, but its registry
index is empty. The CLI can install local package directories; its declared
dependencies currently resolve through the registry. Offline release staging
must install a complete pinned dependency closure without registry access.
The release records the exact selected versions and integrities. Staging is
idempotent: preserve an identical existing version, reject conflicting bytes,
and retain older versions needed by saved inventions. Never overwrite workspace
content. Bundled presence does not require registry publication.
The schema below defines the target; these distribution gaps remain explicit
implementation prerequisites, not capabilities implied by the examples.

## Shared wire schema, version 1

All discovery surfaces use the same entries and references. Tool names are left
to the consumer issues; the operations here are `list`, `detail`, and `load`.
Unknown schema versions fail explicitly. Consumers may ignore additional
response fields; invalid request fields and mixed reference variants fail.

Package listing continues to expose all existing manifest kinds, including
modules, agents, skills, and sample packs. The entries below are its musical
content projection, not a replacement for the package manifest schema. Package
install responses carry the committed catalog generation for subsequent reads.

| Field | Required type and meaning |
| --- | --- |
| `schema_version` | integer `1` on every request/response envelope |
| `generation` | opaque string identifying one daemon catalog snapshot |
| `id` | stable string, independent of display title |
| `version` | exact package SemVer, or workspace revision described below |
| `kind` | `development` or `invention` |
| `name` | display title; never used as a load key |
| `summary` | string; empty when no description exists |
| `source` | `bundled`, `installed`, or `workspace`; provenance, not identity |
| `ref` | one of the two reference objects below, copied unchanged into import/load |

Use the entry document's title for `name`, falling back to the package ID or
workspace path when absent. Package summaries come from the manifest; workspace
summaries come from the document description.

Package reference: `{ "package": "fugue.instruments.pad", "version": "0.1.0" }`.
Both strings are required, with no other fields. `version` is an exact SemVer,
not a requirement. The loader obtains the entry path and kind from the manifest.
It must use an exact requirement internally (`=0.1.0`), since a bare `0.1.0`
in existing package requirement syntax has caret semantics. Package entry paths
are implementation metadata, never something the agent must construct.

Workspace reference: `{ "workspace_path": "voices/my-pad.json", "revision": "sha256:<hex>" }`.
Both strings are required. The revision contains 64 lowercase hex digits.
The path is normalized UTF-8, case-sensitive, slash-separated, relative to the
daemon's existing inventions directory (`~/.fugue/inventions` by default).
Reject absolute paths, empty segments, `.`/`..`, backslashes, and paths escaping
the root after symlink resolution. No client working directory, sibling repo,
or implicit recursive search of the user's home directory is involved.

Workspace discovery enumerates JSON invention documents under that directory,
classifying developments with the existing `Invention::is_development` rule.
Other JSON assets are not catalog entries. This uses the existing workspace;
it introduces neither root registration nor a workspace manifest. A future
configurable root must be selected in daemon context, not guessed by a client.

Workspace entry IDs are `workspace:<workspace_path>`; their versions are the
revision strings. Moving/renaming a file changes its ID. Editing it changes its
revision. Workspace IDs are stable across daemon restarts on that workspace,
not portable identities across machines. Package entry IDs equal manifest IDs
and are portable to any daemon with that exact package version installed.

For a workspace revision, hash a canonical sorted array of pairs
`["file:" + normalized root-relative path, SHA-256 of exact file bytes]` encoded
as compact UTF-8 JSON (no whitespace or ASCII escaping). Sort by the first string
in UTF-8 byte order and include each key once. Include the entry and its
transitive local developments and assets.
Resolve package dependencies to exact versions and include their manifest IDs,
versions, and existing package integrity values as additional sorted pairs
`["package:" + id + "@" + version, integrity]`.
Reject dependency cycles. Revalidate this closure on detail/import; a changed
dependency must not silently change the sound behind an unchanged reference.
Only files in the selected closure contribute, so unrelated edits do not expire it.

Package versions are immutable: changed bytes require a new version. Reuse
package integrity and lockfile machinery to detect a changed installed payload.
An invention document's top-level `version` is its format version and must never
be treated as the package version or workspace revision.

## Selection, collisions, and lifecycle

List all distinct `(id, version)` entries; use filters to narrow them. Two
different IDs may have the same name. A name-only lookup returns `ambiguous_name`
with bounded candidates when more than one matches, never the first match.
Duplicate invention-local type names, including collisions with registered
primitive types, fail import with `duplicate_type_name`; choose a different alias.

An exact reference has no source search order. Workspace references resolve only
in the workspace; package references resolve only in the package cache selected
by the daemon (`FUGUE_PACKS_DIR`, otherwise the existing default). Bundled content
is staged into that cache and uses the same resolver. Identical bundled/installed
copies collapse to one entry; if both provenance records exist, report `bundled`.
Conflicting bytes for the same package ID/version fail as `integrity_mismatch`.
Workspace content never shadows a package, even when its title matches.

For authored dependency requirements, reuse existing package selection: an
installed satisfying lock pin wins, otherwise the highest satisfying installed
version. A frozen load fails on a missing/mismatched pin. Catalog references
always pin exact versions and cannot fall forward to another installed version.
Listing/detail/import never install packages or contact a registry implicitly.

Publish a new generation after a successful install transaction and dependency
closure validation; only then report install success to the catalog consumer.
Failed installs leave the previous snapshot visible. Uninstall and workspace
changes invalidate affected entries. External CLI installs must be detected
before the next list/detail returns, even if the daemon initiated no operation.
Watchers may accelerate this, but cannot be the only correctness mechanism.

Generation tokens are daemon-session scoped. Paginated requests must retain the
same snapshot; expired cursors return `stale_cursor`, prompting a fresh list.
An unrelated install does not invalidate an exact package reference. Removing
that package yields `content_not_found`; changing workspace content yields
`stale_reference`. Recheck at load time, not just at discovery.

## List, detail, and import examples

A bounded list request:

```json
{"schema_version":1,"kind":"development","limit":20}
```

Illustrative response containing the selected pad (other entries omitted here):

```json
{
  "schema_version": 1,
  "generation": "session-a:7",
  "items": [{
    "id": "fugue.instruments.pad",
    "version": "0.1.0",
    "kind": "development",
    "name": "Pad Voice",
    "summary": "A slow, sustained pad with a soft lowpass body and gentle LFO motion in the filter.",
    "source": "bundled",
    "ref": {"package":"fugue.instruments.pad","version":"0.1.0"}
  }],
  "next_cursor": null
}
```

`limit` defaults to 20, permits 1–100, and applies to returned entries. Optional
filters are `kind`, `source`, and exact `id`; `cursor` is opaque. Sort by ID then
SemVer ascending (workspace IDs have only one current revision). Empty results
use `items: []` and `next_cursor: null`. Limit summaries to 512 UTF-8 bytes,
truncating at a character boundary. Responses have a 64 KiB byte cap; return a
cursor before exceeding it. If one entry cannot fit, return `response_too_large`.

Detail request:

```json
{"schema_version":1,"ref":{"package":"fugue.instruments.pad","version":"0.1.0"}}
```

Detail returns `schema_version`, `generation`, the complete list `entry`, and
`interface` for developments. The pad interface is derived from its actual
definition, preserving alias targets and duplicate fan-out declarations:

```json
{
  "inputs": [
    {"name":"frequency","to":"osc","to_port":"frequency"},
    {"name":"gate","to":"sus","to_port":"gate"},
    {"name":"pedal","to":"sus","to_port":"pedal"}
  ],
  "outputs": [{"name":"audio","from":"vca","from_port":"audio"}],
  "controls": [
    {"key":"attack","module":"env","control":"attack"},
    {"key":"release","module":"env","control":"release"},
    {"key":"warmth","module":"filter","control":"cutoff"},
    {"key":"motion","module":"motion_lfo","control":"frequency"}
  ]
}
```

Detail also returns `dependencies` (array of exact content references) and
`assets` (array of paths relative to their declaring document, for inspection
only). For pad both are empty. Detail has a 256 KiB cap and errors explicitly
instead of truncating an interface. Full definition reads are explicit, separately
bounded operations; compact detail never dumps the invention graph.

The caller copies `ref` into this complete proposed invention document. Its
clock triggers a sustained A3 pad through the bundled instrument:

```json
{
  "version":"1.0.0",
  "title":"Pad study",
  "developments":[{
    "name":"my_pad",
    "ref":{"package":"fugue.instruments.pad","version":"0.1.0"}
  }],
  "modules":[
    {"id":"clock","type":"clock","config":{"bpm":60,"gate_duration":0.5}},
    {"id":"pitch","type":"melody","config":{"root_note":57,"scale_degrees":[0],"note_weights":[1]}},
    {"id":"voice","type":"my_pad"},
    {"id":"out","type":"dac"}
  ],
  "connections":[
    {"from":"clock","from_port":"gate","to":"pitch","to_port":"gate"},
    {"from":"pitch","from_port":"frequency","to":"voice","to_port":"frequency"},
    {"from":"pitch","from_port":"gate","to":"voice","to_port":"gate"},
    {"from":"voice","from_port":"audio","to":"out","to_port":"audio"}
  ]
}
```

`frequency` is an input, not an aliased control, so the single-note melody
supplies it through a connection. Listing pad does not make `my_pad` a registered type; loading
this document does. Save/reload retains `ref` and the local name, not a resolved
absolute path or flattened definition.

An independently installed package uses exactly the same flow, for example the
existing `fugue.demo.pulse-development` at `0.1.0`: list reports `source: installed`,
detail accepts `{"package":"fugue.demo.pulse-development","version":"0.1.0"}`,
and import places that object under `developments[].ref` with a chosen name.

Concrete installed entry and import declaration:

```json
{
  "entry": {
    "id":"fugue.demo.pulse-development","version":"0.1.0",
    "kind":"development","name":"Soft Pulse Voice",
    "summary":"A reusable soft pulse voice with frequency and gate inputs.",
    "source":"installed",
    "ref":{"package":"fugue.demo.pulse-development","version":"0.1.0"}
  },
  "detail_request":{"schema_version":1,"ref":{"package":"fugue.demo.pulse-development","version":"0.1.0"}},
  "import_declaration":{"name":"pulse","ref":{"package":"fugue.demo.pulse-development","version":"0.1.0"}}
}
```

For a workspace development at `voices/my-pad.json`, list reports
`id: workspace:voices/my-pad.json`, `source: workspace`, and its computed revision.
Copy the returned workspace reference unchanged into detail and
`developments[].ref`. Do not construct a revision or substitute an agent-host path.
Workspace inventions use the same reference shape with `kind: invention`.

For a byte-for-byte copy of the inspected pad file at that workspace path, the
following groups the list entry, detail request, and import declaration (it is
an example transcript, not a new envelope schema):

```json
{
  "entry": {
    "id":"workspace:voices/my-pad.json",
    "version":"sha256:2b566ae718da4772999a5492ffc023f0b2937b0bf5f6be8e201f7a8a49edaa60",
    "kind":"development","name":"Pad Voice",
    "summary":"A slow, sustained pad with a soft lowpass body and gentle LFO motion in the filter.",
    "source":"workspace",
    "ref":{"workspace_path":"voices/my-pad.json","revision":"sha256:2b566ae718da4772999a5492ffc023f0b2937b0bf5f6be8e201f7a8a49edaa60"}
  },
  "detail_request":{"schema_version":1,"ref":{"workspace_path":"voices/my-pad.json","revision":"sha256:2b566ae718da4772999a5492ffc023f0b2937b0bf5f6be8e201f7a8a49edaa60"}},
  "import_declaration":{"name":"local_pad","ref":{"workspace_path":"voices/my-pad.json","revision":"sha256:2b566ae718da4772999a5492ffc023f0b2937b0bf5f6be8e201f7a8a49edaa60"}}
}
```

List inventions to discover the starter package; its entry has
`id: fugue.starter.bwv772`, `version: 0.1.0`, `kind: invention`, `source: bundled`.
Detail returns the entry, dependencies, and assets, without a development
interface. Load uses the returned reference:

```json
{"schema_version":1,"ref":{"package":"fugue.starter.bwv772","version":"0.1.0"}}
```

The existing load tool gains `ref` alongside `path` and `json`, exactly one of
which must be present. Loading follows existing load/playback semantics;
listing and inspection never load, register, or play anything.

## Resolution and failure behavior

Resolve on the daemon host. Load the selected manifest entry with its actual
source path, then resolve each nested development and asset relative to its own
declaring file. A caller's invention location never becomes a package's base
directory. Relative package files must remain within the package after
canonicalization; cross-package dependencies use exact package references and
declared `deps`, not sibling-directory paths. Workspace catalog closures must
remain within the workspace. Existing explicit local-path loading remains
available separately and does not gain catalog containment rules retroactively.

Package score assets belong in the Bach package, e.g. an authored asset path
`scores/bwv772.score.json` relative to its invention entry. This is a specified
package layout for implementation, not a filesystem path for an agent to guess.
The release closure includes every instrument and asset it references. Dependency
cycles, missing files, invalid documents, kind mismatches, and unsupported module
types fail validation before replacing a live invention. All indexing, hashing,
and I/O runs off the audio thread; failed imports preserve the running graph.

Errors use `{schema_version: 1, error: {code, message, ...}}`; messages explain
how to recover without requiring the client to infer paths. Required codes:

| Code | Meaning / recovery |
| --- | --- |
| `invalid_request` | Invalid schema, filter, limit, or reference; correct request. |
| `content_not_found` | Missing exact package/file; install explicitly or list again. |
| `stale_reference` | Workspace revision changed; inspect and select the new revision. |
| `ambiguous_name` | Multiple names match; select a returned exact reference. |
| `duplicate_type_name` | Local alias already used; choose another name. |
| `kind_mismatch` | Invention requested as development or conversely. |
| `invalid_content` | Parse/schema error, dependency cycle, or unsupported module. |
| `dependency_not_found` | Missing dependency/asset; include declaring ref and dependency. |
| `integrity_mismatch` | Package bytes conflict with recorded version/integrity. |
| `outside_root` | Catalog content/dependency escapes its owning root. |
| `catalog_unavailable` | Root unreadable or refresh failed; never report a false empty list. |
| `stale_cursor` | Snapshot expired; restart list. |
| `response_too_large` | Explicit detail/entry exceeds its byte cap. |

A missing optional workspace/cache directory is empty; a permission or I/O error
is not. Invalid catalog candidates produce bounded `diagnostics` on list
responses and no usable reference; do not silently label the catalog healthy.
Diagnostics use the same error shape, at most 20 per page, with a
`diagnostics_truncated` boolean. Keep diagnostic bytes within the list cap.

## Release acceptance and implementation ownership

On a machine with no source checkout and no network, install the release and
start the daemon from an unrelated directory. The five development packages and
Bach starter closure must already be available in its selected package cache.
Discover pad, inspect its four aliases, import via the returned reference, and
save/reload successfully. Discover and load Bach with all assets/dependencies.
Confirm In C and the unselected examples are absent from bundled discovery.

FUG-251 owns development discovery and reference import support in the shared
loader. FUG-252 owns installation/catalog refresh and coordinating offline
starter staging. FUG-253 owns curated invention discovery and reference load
support, plus arranging/validating the Bach starter content. These require core
library, packs, and release work as appropriate; surface adapters remain thin.
Before implementation, reconcile these prerequisites with those issues rather
than silently claiming current package publication or loaders already suffice.

Required consumer tests cover release/no-checkout use, empty roots, unreadable
roots, duplicate titles/types/identities, exact-version selection, dependency
base directories and cycles, workspace staleness, install rollback/refresh,
pagination across generations, round-trip save/load, and unchanged live audio
after failed loads. Content readiness also requires a listening review of Bach.
