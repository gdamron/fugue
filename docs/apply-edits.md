# Atomic structural edits

`RpcCommand::ApplyEdits { edits }` changes the structure of the running
invention in one step. A batch of edits (add or remove modules, connect or
disconnect ports, set authored control values, move modules on the canvas)
either commits as a whole, in
one revision, or is refused and changes nothing. No audio block ever plays a
half-applied batch.

The batch is applied to the daemon's retained authored document (the document
a save writes), and the change to the running graph is planned from that
document (see [How a batch is planned](#how-a-batch-is-planned)).

Whole-document `ReloadInvention` remains the default structural path. Use it
when you hold the whole document. Use `ApplyEdits` for a targeted change to a
large invention, where sending the whole document back costs more than the
change: a new voice wired into a mixer, a filter inserted into a chain, a few
starting values retuned together.

This document is for authors of adapters (an MCP tool, an editor) that wrap the
command. The adapter needs no validation logic of its own: every check
described here is made by the daemon, and every refusal is structured.

## Request

```json
{
  "schema_version": 1,
  "kind": "command",
  "command": "apply_edits",
  "mutation": { "id": "editor-7f3a-0042", "issued_at": { "session_id": "…", "revision": 12 } },
  "expected_revision": { "session_id": "…", "revision": 12 },
  "edits": [
    { "op": "add_module", "id": "tremolo", "module_type": "lfo", "config": { "rate": 5 } },
    { "op": "connect", "from": "tremolo", "from_port": "bipolar", "to": "lead", "to_port": "amplitude_mod" },
    { "op": "set_control", "module_id": "lead", "key": "amplitude_mod_depth", "value": 0.3 }
  ]
}
```

`edits` is a list of `StructuralEdit`, each tagged by `op`:

| `op` | Fields | Effect |
| --- | --- | --- |
| `add_module` | `id`, `module_type`, `config` (optional, defaults to `null`), `name` (optional) | Adds a module. The id must not exist yet. |
| `remove_module` | `id` | Removes a module and every connection to or from it. |
| `connect` | `from`, `from_port`, `to`, `to_port` | Connects an output port to an input port. |
| `disconnect` | `from`, `from_port`, `to`, `to_port` | Removes an existing connection. |
| `set_control` | `module_id`, `key`, `value` | Sets a control's authored starting value. |
| `move_modules` | `positions` | Places modules on the canvas: changes only the document's `layout`. |
| `name_modules` | `names` | Names modules: changes only each module's `name` in the document. |

Edits apply **in order**, each against the result of the ones before it. A
module added at index 0 can be connected at index 1 and tuned at index 2; a
module removed at index 0 is unknown from index 1 on. To replace a module,
remove it and add it again with the same id; there is no swap op.

`set_control` is always an authoring write: the value becomes the module's new
starting state and is written into its config, exactly as a standalone
`SetControl` with the default `author` intent. There is no `intent` field. Live
performance gestures stay on `SetControl`/`SetControls` with `intent: perform`.

A write to one part of a control, such as a cell sequencer's `cells.2` (one
cell of its `cells` bank, see [module-design.md](module-design.md#a-bank-of-cells)),
is made as a write of the whole: it is built from the whole as the batch has
it so far, so several cells written in one batch all land, and it is
reported in `controls_written` and recorded in the document as the whole
(`cells`). A part that does not fit is refused as `invalid_control_value`.

`move_modules` maps module ids to positions, `{ "x": …, "y": … }` in the
document's canvas units, or to `null` to unplace a module:

```json
{ "op": "move_modules", "positions": { "osc": { "x": 120, "y": 40 }, "vca": null } }
```

It writes the document's `layout` (see the invention format's `layout`
section) and nothing else: no module is rebuilt, no graph change is published,
and nothing heard changes. The revision still advances, so a client tracking
revisions sees the move. Positions are absolute, so the same edit made twice
leaves the same document. A position with a field other than `x` and `y` is
refused when the request is parsed.

A batch of `move_modules` edits alone takes a path of its own: it writes the
layout straight into the retained document, all or nothing, without the
throwaway build described in
[What checking a batch touches](#what-checking-a-batch-touches), and it does
not cancel a timed edit waiting to land. When a timed edit batch or a timed
reload lands, its document keeps the layout as moved meanwhile, with its own
placements made on top: a batch's moves, or each position a reload's
document places differently from the document it was queued against. A
module either removes loses its position.
A `move_modules` cannot be timed on its own: a timed batch must change
structure.

`name_modules` maps module ids to what the piece calls them, or to `null` to
clear a name:

```json
{ "op": "name_modules", "names": { "tone": "Lead filter", "osc": null } }
```

It writes each module's `name` in the document and nothing else, exactly as
`move_modules` writes the layout: nothing is rebuilt, published or heard, the
revision advances, and the same edit made twice leaves the same document.
Commands still address a named module by its id. A name is 1 to 256 bytes, not
blank, with no control characters; `add_module` takes an optional `name` under
the same rules. Moves and names alone take the document-only path together,
a timed batch or reload that lands keeps the names given meanwhile with its
own on top, and neither can be timed on its own. Removing a module drops its
name; `SwapModule` keeps it.

### Arranging a development's inside

`move_modules` with a `development` path places modules inside one
development's definition instead of the piece's own:

```json
{ "op": "move_modules", "development": ["voice"], "positions": { "env": { "x": 120, "y": 40 }, "vca": null } }
```

The path is the one `get_development` takes (see
`docs/authored-inspection.md`): the first name is a development the piece
declares, each next one a development visible inside the one before. Each id
names a module of that development's own definition. The move changes that
definition's `layout` only, so nothing is built, published or heard, and it
runs on the calling control thread. The revision advances, and
`get_development` shows the move at once, no reload needed. Every use of the
development shares its one definition, so every instance shows the new
arrangement.

Where the arrangement is saved depends on who owns the definition, which
`get_development` reports as `saved_in`:

- **Inline in the piece** (`piece`): the definition's `layout` in the
  retained document, so `get_invention` and `save_invention` show it.
- **A development file** (`file`): that file, which every piece using it
  shares. Only the definition's `layout` member changes in the file's text,
  replaced, added or removed in place; every other byte stays as it was. The
  new text is written and synced beside the file with the file's permissions
  before any lock is taken, so a slow disk delays no other control write, and
  then renamed over it, so a reader never sees half of it. A file that cannot be written,
  or that no longer holds what the piece loaded (it changed on disk; reload
  the piece, then arrange again), is refused `development_not_written` with
  nothing changed. The file is read once more just before the rename, but no
  portable rename compares first, so a save by another writer in the moment
  between that last read and the rename is still lost (an accepted, rare
  window, named in `development_file.rs`).
- **An override in the piece** (`override`): a development the piece
  borrows: one from an installed package (a `ref`, or anything declared
  inside one), a file in the package cache, or a file a frozen lockfile
  locks. Its original is never written. The move is saved in the piece's own
  `layout.overrides`, keyed by the path into the development and the module
  id (`"voice/filter"`, or `"voice/env/attack"` one development deeper), and
  `get_development` shows the author's arrangement with the overrides on
  top. `null` removes a module's override, back to where the author put it,
  so one `move_modules` with `null` for every id `get_development` lists in
  `overrides` goes back to the original. A name holding `/` cannot be keyed;
  a move of it is refused `read_only_development`.
- **Read-only** (`read_only`): every development inside a piece that is
  itself an installed package's file (an installed example), which is never
  saved here. A move inside one is refused `read_only_development`, with the
  reason.

Overrides are presentation, as the rest of a layout is: they build nothing,
and they do not raise the document's `minimum_core_version`. An older core
ignores `layout.overrides` as a member it does not know, and loads and plays
the piece as before (saving there drops them). When the original changes (the
piece moves to a new package version), load and reload drop each override
that no longer names a module of a development the piece borrows, with a
warning naming its key, and `get_development` lists the dropped module ids in
`overrides_dropped` until the next load or reload.

A move inside a development applies at once, in a batch of `move_modules` and
`name_modules` alone: with a structural edit, or timed, the batch is refused
`invalid_request` with nothing applied. One batch arranges at most one
development saved in a file, so it stays all or nothing; a second is refused
`invalid_request`. A timed edit batch or timed reload that lands later
keeps the move, unless the reload arranges that development itself; a timed
reload's development definitions are adopted as it lands. A move made just
as a timed reload lands is checked again against the definitions it
brought; should they change once more meanwhile, it is refused
`invalid_request` with nothing applied: read the development again, then
move it again.

Coordinates are numbers: an integer re-saves as a float (`120` becomes
`120.0`), and every value round-trips exactly.

Removing a module drops its position with it, so removing and adding the same
id in one batch leaves the new module unplaced. `SwapModule` keeps the
module's position.

Unknown fields inside an edit, and unknown ops, are refused when the request
is parsed, so a misspelled edit field fails loudly instead of being ignored.
Fields the envelope or the command itself does not know are ignored, as for
every other command.

Edits address the invention's top-level modules. Apart from a
`move_modules` arranging a development's inside, they do not reach inside a
development's definition, and they do not change the document's developments,
assets, title or exposed sections.

### Limits

| Bound | Value | Refusal |
| --- | --- | --- |
| Edits per batch | 1 to 256 (`MAX_EDITS_PER_BATCH`) | `invalid_request`, no edit detail |
| Module id, module type, port name, control key | 1 to 256 UTF-8 bytes (`MAX_EDIT_NAME_BYTES`) | `invalid_edit`, reason `invalid_name` |
| Positions per `move_modules` | 1 to 1024 (`MAX_MOVES_PER_EDIT`), each finite | `invalid_edit`, reason `invalid_position` |
| Names per `name_modules` | 1 to 1024 (`MAX_NAMES_PER_EDIT`), each 1 to 256 bytes (`MAX_MODULE_NAME_BYTES`), not blank, no control characters | `invalid_edit`, reason `invalid_name` |

Split a larger change into several batches. Each batch commits on its own.

### Ticket and revision

A `mutation` ticket is **required**. A request without one is refused as
`invalid_request` before anything runs (`RpcRequest::check_ticket`). Mint the
ticket from the latest revision you have seen, with an id unique to your
client, and reuse it unchanged on every retry of the same batch. See
`MutationTicket` and `MutationLedger` for the full recovery contract.

`expected_revision` is optional. When present, the batch applies only if the
daemon is still at that revision; otherwise it is refused as
`revision_conflict` with a structured `conflict` body and nothing runs. Omit it
to apply against whatever the document is now; the per-edit checks still
protect you from naming a module or port that no longer exists.

## Response

A committed batch answers with `kind: "edits_applied"`:

```json
{
  "schema_version": 1,
  "revision": { "session_id": "…", "revision": 13 },
  "kind": "edits_applied",
  "edit_count": 3,
  "added": ["tremolo"],
  "removed": [],
  "rebuilt": [],
  "controls_written": [{ "module_id": "lead", "key": "amplitude_mod_depth" }],
  "connections_added": 1,
  "connections_removed": 0,
  "untouched": 41
}
```

| Field | Meaning |
| --- | --- |
| `edit_count` | Edits in the batch; all were applied. |
| `added`, `removed` | Module ids added to or removed from the running graph. A module added and removed again in the same batch appears in neither. |
| `rebuilt` | Module ids that existed before the batch and were removed and added again in it. Each gets a fresh instance, even with an identical type and config, so its internal state restarts. |
| `controls_written` | `{ module_id, key }` for each distinct control the `set_control` edits wrote, once, in first-written order. Only modules that exist after the commit are listed (survivors, added and rebuilt modules): a write to a module that a later edit removed or replaced goes with that module. |
| `controls_failed` | Present only when a write failed at commit; see below. |
| `connections_added`, `connections_removed` | Connection counts. Connections that go with a removed module are not counted. |
| `untouched` | Surviving modules that were not rebuilt. They keep their instance, phase and state. A module whose only change is a `set_control` counts here: its value changes, its instance does not. |

The report's size depends on the batch, never on the invention: each list
holds at most one entry per edit. It is not a snapshot; read one if you need
the new state. A field added by a later daemon reads as empty from an older
one.

### A control that fails at commit

Every control value is checked against the module's own rules before anything
is published, so a value the module would refuse is refused with its edit's
index (see below). Rarely, a write that passed that check still fails when the
commit makes it: a sample file that does not load, say. The batch is still
committed: the revision advances and every other edit stands. The failure is
listed in `controls_failed`, the module keeps the value it had, and the
retained document records that value, so a save and the running module agree.
A failed control is not listed in `controls_written` and emits no
`control_changed`.

```json
"controls_failed": [
  { "edit_index": 4, "module_id": "keys", "key": "sample", "error": "file not found: keys/c4.wav" }
]
```

`edit_index` is the last `set_control` edit in the batch that wrote that
control. `error` is the module's reason, cut to at most 256 bytes on a
character boundary.

The envelope's `revision` is the revision the batch committed at. Use it as
the next `expected_revision` and the next ticket's `issued_at`.

## Refusals

A refusal changes nothing: no module, connection or control, no revision
advance, no event, no saved session. Playback continues on the invention as it
was.

### One edit at fault

When a single edit cannot apply, the error code is `invalid_edit` and the error
carries an `edit` detail naming the **first** failing edit:

```json
{
  "kind": "error",
  "code": "invalid_edit",
  "message": "edit 1 (connect) refused: module 'lead' has no input port 'amp' (available: frequency, fm, am); nothing was applied",
  "edit": {
    "index": 1,
    "op": "connect",
    "reason": "unknown_port",
    "message": "module 'lead' has no input port 'amp' (available: frequency, fm, am)"
  }
}
```

`index` is zero-based. Edits after it were not checked. Fix this one and
resend the batch with a **new** ticket (see
[Retrying after a lost reply](#retrying-after-a-lost-reply)).

| `reason` | Raised by | Meaning |
| --- | --- | --- |
| `invalid_name` | any | An id, type, port or key is empty or longer than 256 bytes; or a module name is blank, too long or holds a control character, or a `name_modules` names none or more than 1024. |
| `unknown_module` | any | The module does not exist at this point in the batch (inside a development, in its definition). |
| `duplicate_module` | `add_module` | The id already exists at this point in the batch. |
| `unknown_module_type` | `add_module` | The running invention cannot build this type. |
| `invalid_config` | `add_module` | The module type refused the config. |
| `unknown_port` | `connect` | The source has no such output, or the destination no such input. |
| `connection_exists` | `connect` | The connection already exists. |
| `connection_not_found` | `disconnect` | No such connection exists. |
| `unknown_control` | `set_control` | The module exposes no control with that key. |
| `invalid_control_value` | `set_control` | The value cannot be coerced to the control's kind, is not a finite number, or the module refuses it. |
| `invalid_position` | `move_modules` | It places no module, more than 1024, or one at a position that is not finite. |
| `unknown_development` | `move_modules` | Its `development` path names no development the piece declares. |
| `read_only_development` | `move_modules` | The development is never written here: an installed package's, or a file the package cache holds or a frozen lockfile locks. |
| `development_not_written` | `move_modules` | The development's file cannot be written, or changed on disk since the piece loaded it. Nothing changed. |

Messages that name a port or control list what is available, so an agent can
correct the edit without another read.

`set_control` values are coerced to the control's declared kind first, as a
standalone write is: `"0.5"` becomes `0.5` for a number control, `"true"`
becomes `true` for a boolean one, and `3` becomes `"3"` for a string one. A
value that still has the wrong kind, or a number that is not finite (NaN, an
infinity, or a value too large for a 32-bit float such as `1e39`), is
`invalid_control_value`.

JSON has no NaN or infinity. A Rust client that calls `check_edit_batch`
before sending refuses a non-finite number there, with the same
`invalid_control_value` detail at the edit's index; without that check the
number goes out as `null` and the daemon can only answer with a parse error
that names no edit. When a JSON client sends a number too large for a 32-bit
float, the daemon refuses it at its edit's index as `invalid_control_value`.

The value is then checked against the module's own rules, before anything is
published, exactly as the module's setter would check it. Every write is
checked, so a value the module refuses fails the batch even when a later edit
overwrites it. A value the module
would refuse is `invalid_control_value` at that edit's index: a read-only
control (a sequencer's `cell`, say), text that does not parse as the
JSON a control expects (such as `cells`), or an option the control
does not offer. These are the rules a standalone `SetControl` meets
(see the front doors in [`control-model.md`](control-model.md)): a number
outside the range its control holds is written, and recorded in the
document, as the nearer limit, and its `controls_written` entry says
`clamped: true`; a choice in another case is written as the option it
names.

Writes are checked in batch order, each on the module as the edits before it
leave it, so one write can depend on another. After `degree_count = 7` on a
three-degree melody, `degree.6` exists and can be written; after
`degree_count = 3`, writing `degree.6` is refused at its index, and nothing
is applied. A module that survives the batch is checked as authored, that is,
as its retained document builds it, not as live gestures have since left it.
If a gesture changed it in the meantime (a performed `degree_count`, say), a
write that passed the check can still fail when made, and is reported in
`controls_failed`.

Messages echo at most about 64 bytes of a refused control value. A module's
own reason (a refused config, a refused control value, an edited invention
that does not build) is cut to about 256 bytes.

### The batch as a whole

These refusals carry no `edit` detail:

| Code | When |
| --- | --- |
| `invalid_request` | The batch is empty or holds more than 256 edits; the ticket is missing or malformed. |
| `module_build_failed` | Every edit applied, but the edited invention does not build, or its new graph could not be prepared. No single edit is to blame (for example, removing a module a `control_scheduler` still targets, without writing the scheduler a `schedule` that no longer does). |
| `revision_conflict` | `expected_revision` did not match, or the ticket was issued by another daemon session. |
| `mutation_expired` | A retried ticket is older than the daemon's recovery ledger remembers. |
| `audio_thread_stopped` | Nothing is running, or the audio thread stopped. Load an invention first. |
| `unsupported` | The running invention keeps no authored document to edit. |
| `internal` | The invention changed underneath the batch (a script's edit, say). Either the change would undo a module no edit names, or the graph kept changing through every attempt (see [How a batch is planned](#how-a-batch-is-planned)). The message names the module. Read the invention again and resend. |

## Retrying after a lost reply

If the connection drops before the reply arrives, resend the **identical**
request with the **same** ticket. The daemon never runs a ticketed batch twice:

- If the batch committed, the answer is `kind: "mutation_committed"` with
  `mutation_id` and `committed_at`, the revision it produced. The original
  report is not repeated; read state if you need it.
- If the batch was refused, the answer is the same refusal, whatever the
  cause: `invalid_edit`, `module_build_failed`, `audio_thread_stopped` or
  any other refusal recorded against the ticket. Resending the same ticket
  never retries the batch.
- `mutation_expired` or `revision_conflict` with reason `session_replaced`
  means the outcome is unknown. Re-read the invention (`GetInvention` or
  `InspectInvention`) and decide whether your change is present.

So once you have fixed the cause of a refusal, even one that was not your
batch's fault (an invention that was not running, say), send the batch as a
new command with a **new** ticket. Reuse a ticket only to resend after a lost
reply.

A refused batch is recorded without advancing the revision, so any number of
refusals never expire tickets minted at the same revision.

### An older daemon

A daemon built before `apply_edits` existed cannot parse the request and
answers with a generic request error, never `invalid_edit`. The wire schema
version does not tell the two apart, since the command was added within
schema version 1. Check the daemon's `build` fingerprint from hello
(`DaemonIdentity`) before relying on `apply_edits`.

## What checking a batch touches

Checking a batch builds each module it adds, and then the whole edited
invention (unless the batch only moves modules), as throwaway instances that activate no output: no recorder opens
its file and no stream starts. A recorder that is already running is never
built again over the file it is writing. A `wasm_module` still runs its
constructor, as it does on load.

An added module's `$asset` and audio asset references resolve against the
invention's assets, relative to its file, exactly as on load. The document
keeps the reference as written.

A `set_control` of an asset control (a sample player's `asset`, a kit slot's
or instrument zone's `asset.N`) given a relative path resolves the same way,
in a batch or as a standalone write, so it plays the file the saved document
loads. An invention with no file has nothing to resolve it against, so such a
write is refused: give an absolute path, or save the invention first.

Developments are built from the definitions loaded with the invention, nested
ones included. A development file changed or deleted on disk since then does
not affect a batch; reload the invention to pick up the change.

The modules a batch adds or rebuilds are built for real only once the batch is
planned, just before it is published. A recorder creates its file only once
audio reaches it, so one added by a batch that is then refused (the graph kept
changing through every attempt, say) leaves no file behind.

## How a batch is planned

The change to the running graph is the difference between the retained
authored document and the candidate, both as authored. Only what the batch
changed is planned: a standalone authored `SetControl` made earlier is already
in the retained document, so it is not seen as part of the batch.

A whole-document reload plans differently: it compares the new document with
the configs the running modules were built from, which an authored
`SetControl` does not update. Reload keeps that rule because changing it would
change what reloading an older file sounds like.

The plan may change only modules the batch names (as a module or as a
connection endpoint), and may rebuild only modules the batch removes and adds
again. A plan that reaches further means the invention changed underneath the
batch, for example a script added or removed a module after the batch was
checked. The batch is refused as `internal` rather than undoing that change.

When another change publishes while the batch is planned or prepared, the
batch is planned and prepared again from the same edited document, up to
three times in all, as a reload is. If the graph is still moving after the
third attempt, the batch is refused as `internal` and nothing was applied.

## What a commit does, in order

1. The new graph is swapped in within one audio block. Modules the batch did
   not touch keep running without interruption and keep their phase.
2. Control values written to modules that survive the batch are applied right
   after the new graph is queued. A declared control's write is a control
   request queued behind the new graph, so it applies in the block that
   installs it, never before. A control on a module type not yet moved to
   control requests is written directly, and may be heard up to one audio
   block before the new graph. Added and rebuilt modules are built from
   their final configs and take the batch's values through their setters
   before the new graph is prepared, so they arrive with them: a control's key
   need not be the config key the module is built from (a mixer's
   `level.2` control, say). Writes are made in batch order, as they were checked, so a
   control written more than once ends at its last value.
3. One `control_changed { module_id, key, value }` is emitted per control
   the `set_control` edits wrote, for modules that exist after the commit
   (survivors, added and rebuilt modules). Each carries the control's final
   value, with the same payload as a standalone authored write. Writes to a
   module that a later edit removed or replaced emit nothing. As for a
   standalone write, these are emitted as the change is made, before the
   revision advances.
4. The revision advances once, however many edits the batch held.
5. The session is marked for saving, and a background writer saves it
   shortly after (see [`shared-runtime.md`](shared-runtime.md#publishing-a-graph-change)).
   A failed save is retried and reported, and never undoes the commit.
6. One `topology_changed`. The event carries no graph: a client that wants
   the new one reads a snapshot.

A refused batch emits no events.

## On an offline render

`RenderEngine::apply_edits` (the browser engine's `applyEdits`) takes
`add_module`, `remove_module`, `connect`, `disconnect` and `set_control`,
with the checks, refusals and order above, all or none. It refuses
`move_modules` and `name_modules` for the whole batch, with no edit detail.
It has no revisions, tickets, `when` or events.

A batch with a `set_control` is checked as the daemon checks it, on a copy of
the render's retained document, and its values become the modules' starting
values in that document. The browser engine cannot export that document, so
the report carries `controls_written` and `controls_failed`, with the meanings
above, and `configs`: `{ module_id, config }` for each module the batch
wrote, once, in first-written order, holding the module's config as the
render's document now has it. A host that keeps its own copy of the document
puts each `config` in place of that module's config in its copy. It does not
work out from the edits how a key lands in a config, since a control's key
need not be its config key. A batch without a `set_control` reports
nothing in these fields.

## Not supported

- **Swap.** Remove and add the module instead.
- **Undo.** Send the inverse batch, or reload an earlier document.
- **Partial application.** There is no mode that keeps the edits before a
  failing one.
- **Performance writes.** Use `SetControl` or `SetControls` with
  `intent: perform`.
