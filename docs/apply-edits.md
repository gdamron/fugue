# Atomic structural edits

`RpcCommand::ApplyEdits { edits }` changes the structure of the running
invention in one step. A batch of edits (add or remove modules, connect or
disconnect ports, set authored control values) either commits as a whole, in
one revision, or is refused and changes nothing. No audio block ever plays a
half-applied batch.

The batch is applied to the daemon's retained authored document (the document
a save writes), and the change to the running graph is planned from that
document.

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
    { "op": "add_module", "id": "tremolo", "module_type": "lfo", "config": { "frequency": 5 } },
    { "op": "connect", "from": "tremolo", "from_port": "out", "to": "lead", "to_port": "am" },
    { "op": "set_control", "module_id": "lead", "key": "am_amount", "value": 0.3 }
  ]
}
```

`edits` is a list of `StructuralEdit`, each tagged by `op`:

| `op` | Fields | Effect |
| --- | --- | --- |
| `add_module` | `id`, `module_type`, `config` (optional, defaults to `null`) | Adds a module. The id must not exist yet. |
| `remove_module` | `id` | Removes a module and every connection to or from it. |
| `connect` | `from`, `from_port`, `to`, `to_port` | Connects an output port to an input port. |
| `disconnect` | `from`, `from_port`, `to`, `to_port` | Removes an existing connection. |
| `set_control` | `module_id`, `key`, `value` | Sets a control's authored starting value. |

Edits apply **in order**, each against the result of the ones before it. A
module added at index 0 can be connected at index 1 and tuned at index 2; a
module removed at index 0 is unknown from index 1 on. To replace a module,
remove it and add it again with the same id; there is no swap op.

`set_control` is always an authoring write: the value becomes the module's new
starting state and is written into its config, exactly as a standalone
`SetControl` with the default `author` intent. There is no `intent` field. Live
performance gestures stay on `SetControl`/`SetControls` with `intent: perform`.

Unknown fields inside an edit, and unknown ops, are refused when the request
is parsed, so a misspelled edit field fails loudly instead of being ignored.
Fields the envelope or the command itself does not know are ignored, as for
every other command.

Edits address the invention's top-level modules. They do not reach inside a
development's definition, and they do not change the document's developments,
assets, title or exposed sections.

### Limits

| Bound | Value | Refusal |
| --- | --- | --- |
| Edits per batch | 1 to 256 (`MAX_EDITS_PER_BATCH`) | `invalid_request`, no edit detail |
| Module id, module type, port name, control key | 1 to 256 UTF-8 bytes (`MAX_EDIT_NAME_BYTES`) | `invalid_edit`, reason `invalid_name` |

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
  "controls_written": [{ "module_id": "lead", "key": "am_amount" }],
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
| `connections_added`, `connections_removed` | Connection counts. |
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
control. `error` is the module's reason, cut to about 256 bytes.

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
| `invalid_name` | any | An id, type, port or key is empty or longer than 256 bytes. |
| `unknown_module` | any | The module does not exist at this point in the batch. |
| `duplicate_module` | `add_module` | The id already exists at this point in the batch. |
| `unknown_module_type` | `add_module` | The running invention cannot build this type. |
| `invalid_config` | `add_module` | The module type refused the config. |
| `unknown_port` | `connect` | The source has no such output, or the destination no such input. |
| `connection_exists` | `connect` | The connection already exists. |
| `connection_not_found` | `disconnect` | No such connection exists. |
| `unknown_control` | `set_control` | The module exposes no control with that key. |
| `invalid_control_value` | `set_control` | The value cannot be coerced to the control's kind, is not a finite number, or the module refuses it. |

Messages that name a port or control list what is available, so an agent can
correct the edit without another read.

`set_control` values are coerced to the control's declared kind first, as a
standalone write is: `"0.5"` becomes `0.5` for a number control, `"true"`
becomes `true` for a boolean one, and `3` becomes `"3"` for a string one. A
value that still has the wrong kind, or a number that is not finite (NaN, an
infinity, or a value too large for a 32-bit float such as `1e39`), is
`invalid_control_value`.

The value is then checked against the module's own rules, before anything is
published, exactly as the module's setter would check it. A value the module
would refuse is `invalid_control_value` at that edit's index: a read-only
control (a sequencer's `current_cell`, say), text that does not parse as the
JSON a control expects (such as `sequences_json`), or an option the control
does not offer. Within those rules a module may still clamp a number into its
range or accept an alias for an option, as it does for a standalone write.

Messages echo at most about 64 bytes of a refused control value; a module's
config error is cut to about 256 bytes.

### The batch as a whole

These refusals carry no `edit` detail:

| Code | When |
| --- | --- |
| `invalid_request` | The batch is empty or holds more than 256 edits; the ticket is missing or malformed. |
| `module_build_failed` | Every edit applied, but the edited invention does not build, or its new graph could not be prepared. No single edit is to blame (for example, removing the last output sink). |
| `revision_conflict` | `expected_revision` did not match, or the ticket was issued by another daemon session. |
| `mutation_expired` | A retried ticket is older than the daemon's recovery ledger remembers. |
| `audio_thread_stopped` | Nothing is running. Load an invention first. |

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

## What a commit does, in order

1. The new graph is swapped in within one audio block. Modules the batch did
   not touch keep running without interruption and keep their phase.
2. Control values written to modules that survive the batch are applied right
   after the new graph is queued. A value may therefore be heard up to one
   audio block before the new graph. Values for added or rebuilt modules are
   part of their config and arrive with them. A control written more than
   once ends at its last value.
3. The revision advances once, however many edits the batch held.
4. The session is saved. A failed save is logged and does not undo the
   commit.
5. Events, in this order:
   - one `control_changed { module_id, key, value }` per control the
     `set_control` edits wrote, for modules that exist after the commit
     (survivors, added and rebuilt modules), carrying the control's final
     value with the same payload as a standalone authored write. Writes to a
     module that a later edit removed or replaced emit nothing;
   - one `topology_changed`;
   - one `snapshot`.

A refused batch emits no events.

## Not supported

- **Swap.** Remove and add the module instead.
- **Undo.** Send the inverse batch, or reload an earlier document.
- **Partial application.** There is no mode that keeps the edits before a
  failing one.
- **Performance writes.** Use `SetControl` or `SetControls` with
  `intent: perform`.
