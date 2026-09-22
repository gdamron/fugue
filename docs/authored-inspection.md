# Bounded authored inspection

`RpcCommand::InspectInvention { query }` reads the retained document used by
`GetInvention`. The product exposes the same query as `inspect_invention` over
MCP. This is control-thread work; it does not read the flattened graph, resolve
files, update controls, advance the authoring revision, or emit topology events.
An idle daemon returns `AudioThreadStopped`.

## Start with a musical question

For a layered arrangement, find a voice, read its wiring and starting controls,
inspect the development that gives those controls meaning, then inspect its
upstream sequencer. The minimum query vocabulary is:

| Selection | Contents |
| --- | --- |
| `{"kind":"overview"}` | Title/version/description, module id/type inventory, development declarations, counts for other authored sections |
| `{"kind":"module","id":"voice_17"}` | Exact module, all incident connections, peer id/type summaries, exposed aliases, declared development and `$asset` references |
| `{"kind":"development","name":"voice"}` | Declaration, local instances with wiring/peers/aliases, and inventory of its inline definition |
| `{"kind":"value","pointer":"/modules/37/config/sequences/0"}` | Immediate object members or array elements, or the scalar itself |

Musical selections accept `scope`, defaulting to the root (`""`). Nested scopes
are authored JSON pointers such as `/developments/0/definition`. Module ids are
local to that scope. These are declarations, not expanded instance paths.
Every entry includes its original JSON pointer; use it for further inspection.
JSON pointer escaping uses `~0` for `~` and `~1` for `/`.

`module` includes locally declared shared JSON assets referenced recursively in
its config, and its development declaration (including earlier ancestor
registrations inherited by inline definitions). Other references, including
audio asset paths/catalog refs, remain unchanged inside the authored config.
A peer summary supplies an id and type to follow with another module query.

## Bounds, omissions, and continuation

A successful page is at most **16,384 UTF-8 bytes of compact JSON**, including
revision, source context, entries, and continuation. `limit` defaults to 50 and
must be 1–100 entries; the byte bound can shorten a page further. Individual
entry values are limited to **4,096 compact JSON bytes**. Selectors are limited
to 2,048 serialized bytes. Invalid selectors fail explicitly. Oversized metadata
that cannot fit even one record returns `ResponseTooLarge` with guidance.

Each entry's `coverage` is explicit:

- `complete`: exact value at the pointer, including a real JSON null.
- `summary`: intentionally reduced inventory/peer/development data. Config,
  inline body, or section contents are not included. Read `value` at the pointer.
- `omitted`: value exceeds the individual budget; no `value` field is sent.
  Read `value` at that pointer to split it into immediate children, recursively.
- `fragment`: a long string read via `value`, with a `string_range` of half-open
  UTF-8 byte offsets. Concatenate in range order. Even strings containing
  Unicode or JSON escape characters are losslessly readable in bounded pages.

`total_entries` counts the selection's records, and `offset` identifies this
page's starting record. A non-null `next_cursor` identifies unreturned records.
Repeat the **same selection** with that cursor; changing limit is allowed.
No continuation means enumeration has ended, not that summaries/omissions have
magically become complete. These pages are never replacement invention JSON.
For an empty object/array, `value` returns zero entries.

The MCP tool returns one compact text copy, without duplicated structured
content. Its serialized result envelope is separately capped at **33,792 bytes**
(2 × payload budget + 1,024). The core budget excludes RPC framing and outer
string escaping. Bounds constrain response/context size, not total daemon work:
the implementation currently clones and serializes the retained document on the
control thread before selecting records.

## Consistent reads and source context

Every page carries `revision: {session_id, revision}` and the original daemon
`source_path`. A continuation binds both its selection and revision. Reusing it
after authoring or a daemon replacement returns the shared `RevisionConflict`.
When switching selections, echo the preceding page's revision in the RPC
request's `expected_revision` (the MCP tool exposes this argument). A conflict
means restart/reconcile the inspection; never combine different tokens as a
single snapshot. Perform gestures do not change authored state or its revision.

External development/asset declarations are returned as authored paths/refs.
Their contents are **not** part of the retained document and are not covered by
its revision. An external development has no inline scope. Follow catalog refs
with `DescribeDevelopment`/`describe_development`; daemon-local paths require
explicit source access. Do not interpret independently loaded files as part of
the same revision. Snapshot/file delivery remains `GetInvention`'s responsibility.

## Reproducible context evaluation

`rpc::inspection::tests::layered_arrangement_context_and_call_count` builds 32
voices, 32 long sequencers, clock/mixer, inline voice definition, shared asset
references and incident connections. It reads an overview, selected voice,
development, selected sequencer, its config and all cells, and the voice's
internal envelope. It follows every continuation and reports bytes and calls
against a full snapshot, including a compact full-document baseline to avoid
crediting inspection merely for removing whitespace. Byte/4 token estimates are
explicit estimates, not tokenizer measurements. Full retrieval is one transfer;
its oversized inline path is unsupported, and file delivery needs filesystem
access plus a local read. The benchmark compares document context residency,
not daemon CPU cost, audio continuity, or model editing accuracy.

The reference core run returned 48,440 compact payload bytes in 9 calls, versus
205,565 compact bytes or 564,688 pretty bytes for the full snapshot. The actual
product MCP harness (which includes response envelopes and its longer temporary
source path) returned 58,367 bytes in 9 calls versus 205,653 compact full-snapshot
bytes: about 72% less context, at the cost of eight additional retrieval calls.
The full pretty snapshot was 564,776 bytes. The nine calls include continuations;
no selected sequence steps are left unread in this comparison.
