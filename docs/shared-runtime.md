# The shared runtime contract

One daemon plays the music, and many clients drive it: the CLI, the MCP
adapter, an editor, an agent's conducting script. A client may attach to a
daemon it did not start, edit an invention another client is also editing,
and lose a reply halfway through an edit. This document describes the rules
that keep clients from corrupting each other's work or the music, and why.
The types and rules live in the core crate, under [`src/rpc/`](../src/rpc/);
hosts and clients stay thin transports around them, so every surface applies
the same rule.

See also [`apply-edits.md`](apply-edits.md) (the atomic edit batch, its
refusals and retries), [`authored-inspection.md`](authored-inspection.md)
(bounded reads bound to a revision) and [`control-model.md`](control-model.md)
(control requests, timing, and author/perform intent).

## Who you are talking to

### The handshake

A client's first request is `Hello`. The daemon answers with its
[`DaemonIdentity`](../src/rpc/identity.rs):

| Field | Meaning |
| --- | --- |
| `schema_version` | The wire schema the daemon speaks. |
| `build` | A `BuildFingerprint`: `crate_version`, plus `git_sha` and `dirty` when built in a checkout. |
| `session_id` | Opaque, minted fresh each time a daemon process starts. |
| `pid` | The daemon's process id, for diagnostics and stop guidance. |

`Hello` is answered before the request's `schema_version` is checked, so a
client on a different schema still learns which daemon it reached and can
say so, instead of getting an opaque refusal.

The client then calls `verify_daemon_identity`, which reports a schema
mismatch first, then a build mismatch. The schema alone is not enough: a
rebuilt daemon can keep it and still behave differently, and a dirty build
differs from a clean one of the same commit. On a mismatch the client
**refuses to drive the daemon** and tells the person how to stop it or update
the client. It never takes the daemon over or restarts it: another client may
be mid-performance on it, and cutting that off is worse than asking a person.

The fingerprint deliberately has **no build timestamp**. The CLI and the MCP
adapter are compiled separately; two binaries built from identical source
must fingerprint equal, or they could never share a session. Comparison is
exact equality over all three fields, so a build that reports no `git_sha`
(a packaged build) does not match a checkout build of the same version.

### Lifecycle

A daemon a client spawns **outlives that client**. It is a shared runtime, so
one surface exiting must never silence another's music. Stopping it is
explicit: `RpcCommand::Shutdown`, or an interrupt or terminate signal. Either
way the daemon saves the session before it exits, and refuses new mutations
while it closes. A client whose job is to stop the daemon may skip identity
verification, since the daemon it needs to stop is often the mismatched one.

## Revisions

### What a revision is

A [`RuntimeRevision`](../src/rpc/revision.rs) is `{session_id, revision}`:
a counter scoped to the daemon session that minted it. Because `session_id`
is the identity's, a replacement daemon voids every token its predecessor
issued, even when the counters coincide. Compare tokens for equality only;
never order them.

Every response carries the daemon's current revision in `revision`: reads,
acknowledgements and errors alike. A client always holds a fresh token for
its next edit, and a refused client learns where the daemon stands without
a second round trip.

### What advances it

The revision tracks **authoring**, not **performance**. A control write
carries an intent: `author` makes the value the invention's new starting
state, `perform` is a live gesture that is applied and announced but never
recorded (see [`control-model.md`](control-model.md)). If performed writes
advanced the revision, a scheduler writing at musical rate would make every
peer's structural edit permanently stale.

`RpcCommand::advances_revision` is the single definition:

| Command | Advances |
| --- | --- |
| `LoadInvention`, `LoadExample`, `UnloadInvention`, `AddModule`, `RemoveModule`, `Connect`, `Disconnect` | Always. |
| `ReloadInvention`, `SwapModule`, `ApplyEdits` | When immediate. A timed one (with `when`) advances when it is adopted, not when it is queued. |
| `SetControl` | When its intent is `author`. |
| `SetControls` | When any write in it is `author`. |
| Reads, `SaveInvention`, `FlushSession`, package commands, `Shutdown` | Never. |

A host does not call that directly once a command has run; it calls
`RpcCommand::advances_revision_after(committed)`. Most authoring commands
advance on **any attempt**, even a refused one, because a batch that fails
partway keeps the writes it made first. A spurious advance costs a peer one
re-read; a missed one would let a stale edit overwrite real changes. An
all-or-nothing command (`ApplyEdits`, per `is_all_or_nothing`) advances only
when it commits, because its refusal changed nothing.

### Preconditions

A request may carry `expected_revision`. The daemon then applies it only if
it is still at exactly that revision. A mismatch is refused before the
command touches the runtime, the retained document, the session file or the
event log, as `revision_conflict` with a `RevisionConflict` body:
`expected`, `current` and a `reason`. The `reason` is `stale_revision` (same
daemon, the document was authored since you read it) or `session_replaced`
(a different daemon minted your token).
Omitting `expected_revision` makes the call unconditional, as for clients
that predate revisions. There is deliberately no merge: a refused client
re-reads and decides; reconciling is the client's business.

## Recovering from a lost reply

A client that loses its connection between sending an edit and reading the
reply cannot tell whether the edit landed. Resending blindly can apply a
structural edit twice, or deliver it to a replacement daemon that never saw
the first. The recovery contract lives in
[`src/rpc/recovery.rs`](../src/rpc/recovery.rs);
[`apply-edits.md`](apply-edits.md#retrying-after-a-lost-reply) walks through it
for one command.

### Tickets

An authoring command may carry a `mutation` ticket, a `MutationTicket`:

- `id`: client-minted, 1 to 128 bytes, unique among every client sharing
  the daemon (include something process-unique).
- `issued_at`: the latest revision the client had seen when it minted the
  ticket. Its session fences the command to that daemon; its counter bounds
  how far back the daemon must remember.

A retry resends the command with the **same** ticket. A ticket is optional,
except on an all-or-nothing command, which `RpcRequest::check_ticket`
refuses without one: a lost reply to such a command can then always be
resolved by replaying it. A ticket on a command that does not author is
refused as `invalid_request`.

The daemon answers `Hello` at once, then checks the schema version,
`check_ticket`, the ticket against the `MutationLedger` (below), and the
`expected_revision` precondition, in that order. Only then does the command
run; the revision advances if `advances_revision_after` says so, and the
outcome is recorded.

The ledger comes before the precondition on purpose. A retry of an edit that
committed would otherwise be refused as stale, by the very revision it
produced, and the client could not tell that it had landed.

### The ledger

`MutationLedger::admit` answers a ticketed command one of these ways:

| The daemon finds | Answer |
| --- | --- |
| `issued_at` from another daemon session | `revision_conflict`, `session_replaced` |
| `issued_at` ahead of the daemon's revision | `invalid_request` |
| the id, recorded for the same command | the original outcome; nothing runs again |
| the id, recorded for a different command | `invalid_request` |
| no record, and the ticket is at or before the horizon | `mutation_expired`; nothing runs |
| no record, and the ticket is after the horizon | the command runs and is recorded |

A committed original is answered with a compact `mutation_committed`
(`mutation_id`, `committed_at`), never the original payload, which may be a
whole snapshot. A refused original is answered with the same refusal, its
messages cut to 4 KiB. A queued timed edit is answered with the request
status it was first given, never a second request.

The ledger holds the outcomes of the last **128** ticketed commands that ran
(`MUTATION_LEDGER_CAPACITY`), for the life of the daemon session. There is no
time-based expiry. Refusals made before a command ran (a stale precondition,
a replaced session, an expired ticket) are not recorded; resending one is
refused the same way again.

### The horizon

Expiry is decided by revision, not by time. When the ledger evicts the
outcome of a command that **advanced the revision**, its horizon moves to the
revision that command ran at. A ticket issued after the horizon cannot belong
to a forgotten command that changed state, so running it now is safe. A
ticket at or before the horizon might, so it is refused rather than risk
running twice.

Evicting a command that did not advance leaves the horizon alone. That is
what keeps refusals from expiring live tickets: if every eviction moved it,
129 refusals at one revision would push the horizon up to the current
revision and every fresh ticket would expire. So a host must advance the
revision for every admitted command that may have changed state, partial
failures included, and a command recorded without advancing is taken to have
changed nothing. A queued timed edit may change state later, so it counts as
advancing until the daemon reports its request ended without being adopted
(`MutationLedger::request_unchanged`). In practice a retry expires only when
more than 128 ticketed edits ran between its `issued_at` and the retry.

A ticket expires by its `issued_at`, not by when it was sent. A client that
mints from a revision it saw long ago can be refused as expired on its
**first** send. Only the client knows that send was the first; when it was,
nothing with that ticket ran, so it may mint a new ticket from the refusal's
revision and send once more. The MCP client does this.

### What a client may resend

`RpcRequestPayload::replay_policy` is the single classification every
transport uses after reconnecting:

| `ReplayPolicy` | Requests |
| --- | --- |
| `Resend` | `Hello`, snapshots, event polls, meters, every read command, `SaveInvention` (it writes the current document again), `FlushSession` (fenced to the session it names). |
| `ResendWithTicket` | Every authoring command, timed edits included: only with its original ticket, and only to the session that ticket names. |
| `Never` | Performed control writes, package installs, `Shutdown` (it could stop a replacement daemon), `Subscribe`. |

Performed writes are never resent by the transport, because only the caller
knows what a value means. An adapter holding an absolute, continuous value (a
fader position) may send its latest value again after a fresh handshake shows
the same session. A discrete trigger (a note, a restart, a toggle standing in
for a button) is never replayed, because firing it twice is audible. Neither
is replayed into a replacement daemon: the gesture belonged to a runtime that
no longer exists.

Anything else leaves the outcome unknown, and the client re-reads state
(`GetInvention`, `InspectInvention` or a snapshot). A replacement daemon
restores the last saved session, so an edit that committed before a restart
may or may not be there; only a re-read can tell.

## Publishing a graph change

Every structural change to a running invention goes through one
`Publisher` ([`src/invention/publish.rs`](../src/invention/publish.rs)):
reload, `ApplyEdits`, swaps, single adds, removes and connections, and edits
made by scripts and agents. Sharing one mechanism means no path can play a
half-applied graph.

1. **Prepare**, on a control thread. Everything that can fail happens here:
   modules are built, edits validated, the complete next topology compiled.
   Nothing audible changes.
2. **Publish.** The prepared topology goes to the audio thread on the same
   queue as control requests, so edits and requests apply in the order they
   were submitted. The audio thread installs it at the start of one block,
   or for a timed edit at the sample it lands on, without allocating,
   freeing or locking
   ([`graph/publication.rs`](../src/invention/graph/publication.rs)).
   Surviving modules keep their instance, phase and state.
3. **Commit**, on the control thread. The runtime's view of modules,
   connections, controls and the retained document updates together. The old
   structures come back on a bounded ring and are freed by a reclaimer
   thread, never on the audio thread.

If another change publishes while a reload or edit batch is being prepared,
it is prepared again from the same document, up to three attempts, then
refused as `internal`.

Control values that a change writes to **surviving** modules (a reload's
config changes, an edit batch's `set_control`) are written right after the
publication is queued. A declared control's write is a control request queued
behind the edit, so it applies in the block that installs the new topology,
or later if the queue is under pressure, never before. A control on a module
type that has not yet moved to control requests is written directly, and may
be heard up to one block before the new topology. Added and rebuilt modules
are built with their final values, so they arrive with them.

What follows a commit:

- **Revision.** `ApplyEdits` advances once, however many edits it held, and
  only when it commits. Reload advances on any attempt (see above).
- **Saving.** The daemon marks the session for saving and replies; a
  background writer saves it shortly after. A reply means the change is
  applied and live, not that it is on disk. `FlushSession` waits for disk,
  and every status reports how far saving lags
  ([`durability.rs`](../src/rpc/durability.rs)). A failed save never undoes
  the change or interrupts the music; the daemon retries and reports it.
- **Events.** `ApplyEdits` emits one `control_changed` per control it wrote,
  then one `topology_changed`. A reload's carried values emit no
  `control_changed`, since they reconstruct state rather than perform it.
  `topology_changed` carries no graph, so the polled event log stays small
  whatever the invention's size. A streaming subscriber receives a
  `snapshot` built for it right after; a poller reads one with
  `GetSnapshot` ([`event.rs`](../src/rpc/event.rs)).

## Naming wire encodings up front

A high-rate payload names its encoding in its stream's metadata from its
first version, even while only one encoding exists. A spectrogram stream's
`SpectrogramStreamMeta` carries `encoding`
([`spectrogram.rs`](../src/rpc/spectrogram.rs)):

- `f32_json`: decibel values as a JSON array, simple to read by eye.
- `u8_base64`: one byte per value, about a fifth the size. The default.

The named seam is why the compact form could arrive as an addition rather
than a schema version bump. An encoding a client does not know still parses,
as `unsupported`, so an old client reports the stream as unsupported instead
of misreading its tiles. The frequency axis's `spacing` follows the same
rule, as does `MutationReply`, whose unknown shapes are answered as a
snapshot.

Ship the simple encoding first, and measure before adding a compact one. Any
new display-rate or telemetry payload follows this rule.

