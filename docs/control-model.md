# Control model

Every change to a running invention's controls is a **request**: a typed
value submitted to a queue and applied by the audio thread at a chosen
sample. RPC clients, MCP agents, conducting scripts, reloads, `ApplyEdits`
and the control scheduler all reach module state this way. The audio thread
is the only thread that mutates a module's control state.

This document is for contributors to the engine and reviewers of changes
that touch controls. It explains the model and why it is shaped this way.
The code lives in [`src/control_request/`](../src/control_request/); the
audio-thread drain is in
[`src/invention/graph/requests.rs`](../src/invention/graph/requests.rs).

## Why requests

- **Clean audio.** A control written directly from a control thread needs a
  lock or a multi-word protocol shared with `process()`, and a lock the
  audio thread waits on is a dropout. With one mutator, `Module::apply`
  is a plain setter: no lock, no atomics, no allocation.
- **Sample accuracy.** A write lands on the sample it names, not at
  whichever block boundary the control thread happened to beat.
- **One order.** Every writer goes through one queue, so "last write wins"
  has one meaning, and structural edits are ordered against control writes
  (see [One order with edits](#one-order-with-edits)).
- **One place to decide conflicts.** When a user, an agent and automation
  write the same control, the audio thread weighs them by rank at the
  sample they meet, rather than whichever thread stored last.

## A request

A [`Request`](../src/control_request/request.rs) carries:

| Field | Meaning |
| --- | --- |
| `target` | A module (by position in a graph generation) and a `ControlIndex` in its table |
| `value` | An `RtValue` (`Copy`: f32, u32, i32, bool), a heavy `Payload`, or a structural edit |
| `when` | `Now`, `AtSample`, `AfterSamples`, `AtTime` (wall clock), `Beat`, or `Condition` |
| `intent` | `Author` or `Perform` (see [Intent](#intent-author-or-perform)) |
| `source`, `priority` | Who wrote it (user, agent, script, automation) and a rank within that tier |
| `ttl` | How many samples it may wait before it is refused `Expired` |
| `event` | Whether the write fires (a trigger) rather than sets a state |

Keys are resolved and values coerced on the control thread, so no string
ever reaches the audio thread. Every request leaves the audio thread's hands
exactly once, through `Outcomes::settle` in
[`pending.rs`](../src/control_request/pending.rs), as `Applied`,
`AppliedLate`, `Superseded`, `Overridden` or `Refused(reason)`, and the
outcome goes back on a bounded channel. When that channel is full, the
outcome is dropped and counted rather than block the audio thread.

## The path

1. A front door resolves the key against the module's `ControlTable` and
   coerces the value. A bad key or value is refused here, synchronously.
2. [`RequestSender::submit`](../src/control_request/sender.rs) resolves
   relative times (`AfterSamples`, `AtTime`, `ttl`) against the transport's
   published sample count and pushes onto one bounded, allocation-free MPSC
   queue ([`queue.rs`](../src/control_request/queue.rs), Vyukov's bounded
   queue, model-checked with loom). A full queue returns `QueueFull` to the
   submitter at once.
3. The audio thread drains the queue at each block into a fixed-size
   pending store, sorted by due sample.
4. The graph splits the block into segments at due samples and applies each
   request between `process()` calls through `Module::apply` (or
   `Module::apply_payload`), exactly at its sample.

A request whose sample has already passed applies at the next segment start
and reports `AppliedLate`. A full pending store applies back-pressure: the
drain stops popping, requests wait in the queue in order, and submitters
eventually get `QueueFull`. A request is refused `PendingFull` only when it
cannot wait there: when an edit is queued behind a full store (the requests
ahead of it are refused so the edit is not held up), when the beat-watch or
condition tables are full, or on a drain with no clock (the null backend).
Nothing is silently dropped.

One MPSC queue rather than a ring per producer, because producers come
and go, and merged rings have no single arrival order for "last write wins".

### One order with edits

Structural edits (`ApplyEdits`, reload, module swaps) ride the same queue
as a prepared publication. Requests submitted before an edit act on the
graph it replaces; those after act on the graph it installs. Pending
requests are mapped onto the new graph as it installs, and refused
`TargetGone` when their module was removed or rebuilt. Part of the queue is
reserved for edits, so control traffic cannot starve one.

An edit may be applied at once, or timed like a control request on a
sample, a beat or a condition (`swap_module_at`, `apply_edits_at`,
`reload_at`; the wire's `EditWhen`). A timed edit's document change is
recorded when it lands, not when it is submitted; its status (`queued`,
`landed`, `adopted`, `refused`, `expired`, `cancelled`) is readable and
announced as it changes. See [`apply-edits.md`](apply-edits.md).

## Declared controls

A module on the request path declares its controls once, as a
[`ControlTable`](../src/control_request/declare.rs) of `ControlDecl`s:
kind (number, integer, bool, choice, payload), default, clamp range, unit,
editor hints, and whether a write is an event.

- **Key validity is fixed when the table is built.** An indexed control
  declares `key.0` to `key.{count-1}` for the module's life, whatever
  another control later makes audible. A write to a position that is
  currently hidden is stored, not refused. Validity never depends on
  runtime state, so a key resolved on the control thread is still valid
  when the audio thread applies it.
- **Each control has one writer class** (`Writer`): a *parameter* is
  written by requests; *telemetry* is written only by the module's own
  `process()` (or the one host thread that runs it, as for an agent's
  worker) and refuses every external write as read-only.
- **Events never coalesce.** Two triggers at one sample are two events.
  Automation, a scheduler's writes from the audio thread, is the
  exception. It keeps one write per control per block, so a schedule
  entry on an event (a cell sequencer's `next_cell`, a clock's `reset`)
  fires it once. A module whose every event must play, such as an
  instrument whose notes form chords, queues its automation events
  instead: each applies, in the order written. A ramp on an event is
  refused, since it would fire on every block it crosses.
- **At one sample, values apply before events.** Writes due at the same
  sample all apply, values (states, payloads) first and then events,
  whatever order they were written in: a note off written just before a
  release, or just after it, takes the new release. Writes due at
  different samples apply in time order, inside a block too, and a value
  never reaches back to an event due before it, even when both arrive
  late: the graph finishes each late event as it applies. The request
  store and a module's automation slots each order their own writes so. Some writes
  still apply in the order made:
  - an offline render's writes, each applied at once;
  - requests timed in beats that fall due together;
  - a request and a scheduler's write at one sample, the request first.

  A module whose event reads a value as it sounds, rather than as it
  applies, keeps the rule on every path: a sample instrument's note off
  takes the release in force as the note next renders. A scheduler playing its own table
  (one not stepped by the graph's drain) lands its block's writes on its
  targets together, at the block's start.

Every module type declares its controls; a catalog test
(`src/control_request/catalog_tests.rs`) fails a built-in type that does
not. Only a declared control can be scheduled. A surface may still keep
undeclared controls for host state that no audio thread reads (an agent's
prompt, a script's status): a control thread writes them through their
setters, and they get none of the request semantics (timing, conditions,
holds, outcomes).

## Heavy values

Schedules, patterns, banks, samples and prepared edits are too big to copy
into a request. A [`Payload`](../src/payload.rs) carries one by pointer:

1. A control thread builds the value whole and immutable.
2. It moves (never clones) into a request. The queue's release/acquire
   handoff publishes it to the audio thread.
3. On the audio side it either becomes the module's value (`Shared<T>`) or
   is retired. The value it replaces is retired too.
4. Retired values go on a wait-free SPSC ring
   ([`spsc.rs`](../src/spsc.rs)) to the `Reclaimer`, which drops them on a
   control thread every few tens of milliseconds.

So the last owner of a heavy value always drops it off the audio thread;
debug builds panic if one is dropped inside the audio-thread scope. When the
reclaimer stalls, the drain stops popping at the first payload request
rather than refuse it (refusing would itself need a retirement). That request
and everything behind it, edits included, wait in the queue in order, and
back-pressure reaches submitters as `QueueFull`.

## When a request applies

Requests are timed on the engine's sample transport, a count of samples
since startup. Musical time (beats on a clock, conditions on the running
invention, and which clock a module counts on when a request names none) is
covered in [`musical-time.md`](musical-time.md).

A request that can never fire is refused when that becomes known, never
left waiting: `TargetGone`, `TimelineGone` (its clock went away),
`SourceGone` (a module its condition reads went away), `ChainBroken` (the
request it waited on settled without applying), or `Expired` (its `ttl` ran
out). A `ttl` is how a request on an open-ended condition says how long it
is still wanted.

A request timed in beats may carry a **ramp**: from the control's value at
the beat it glides linearly to the request's value over that many beats,
arriving exactly on the sample the clock reaches the end
([`ramps.rs`](../src/control_request/ramps.rs)). Only linear ramps exist;
other ramp shapes, and ramps given as arrays of timings, are a possible
later extension.

## When writes conflict

[`hold.rs`](../src/control_request/hold.rs) decides who wins a control.

- **Rank.** A write ranks by its source's tier, user = agent > script >
  automation, then by its `priority` within the tier. At one sample the
  higher rank wins whatever the order; at equal rank the later write wins.
  The loser reports `Overridden` (lost to a higher tier) or `Superseded`
  (lost within its tier). The pending store keeps at most one request per
  control per sample.
- **A late write is weighed at its due sample**, so arriving late never
  lets it beat a higher-ranked write that would have followed it.
- **Holds.** A write above the automation tier holds its control against
  automation until automation's next scheduled event on that control. A
  ramp running when the hold is taken is abandoned, not paused; the next
  schedule entry frees the control and ramps on from where the hold left
  it.

**Why.** A performer who grabs a knob that automation is moving keeps it
until the automation's next deliberate move. Holding forever would silently
break the composition; yielding to the next ramp step would make the
gesture inaudible.

A hold covers the target control and the controls a write to it reaches:
a development's exposed control and every inner control it aliases, or
the controls of its own module a write rewrites (a whole-scale
replacement rewrites every scale position). Such a write is weighed
against a hold on any of them, and the strongest loss decides. A control
that only brings others into play rewrites none of them. So when a
scheduler ramps a count while a higher tier holds one of the positions it
reveals, the ramp runs on and the held position keeps its value: the hold
wins, and nothing re-derives it. Events are never held. Hold state is one word per declared control,
allocated when the module is built, and belongs to the instance: a module
that survives an edit keeps its holds; one rebuilt starts with none.

## Automation on the audio thread

A control scheduler runs inside the graph, so it cannot submit to the queue
(submitting takes the publisher's lock). Instead it writes its targets'
**automation slots**
([`automation.rs`](../src/control_request/automation.rs)), one per declared
control, and the graph applies each module's slots through the same
`apply_declared` path just before that module processes. In an invention's
top level, a schedule on a clock is played in place by the graph's request
drain, so each entry lands on the exact sample of its step.

- A slot holds only the latest write, so automation coalesces to the last
  value per block, or per sample inside a feedback group.
- A module may queue its event controls' writes instead
  (`ControlCells::queuing_events`): every one is kept, in write order, and
  applied after its slots. The queue is bounded; a write finding no room
  is refused and counted. Its last places can be kept for one control, a
  release, so note ons crowding it cannot strand the notes they start.
- A write to a held control is dropped and counted.
- A refused write is counted, never formatted: no allocation on the audio
  thread for an error message.

## Control writes are ordering dependencies

A module that writes another module's controls while processing declares
its targets with `Module::control_targets` (mirrored on its
`ControlSurface`). The process-order compiler
([`compile.rs`](../src/invention/graph/compile.rs)) adds each as an
ordering-only edge: it joins the adjacency for ordering but routes no
signal.

- The writer runs before its targets in every block, so a write is never a
  block late.
- A write upstream that closes a cycle (a scheduler writing the tempo of the
  clock that drives it) folds the modules into a feedback group, processed
  sample by sample.
- A live schedule write that changes which modules a scheduler targets
  publishes the topology recompiled with the new order, carrying the new
  schedule, so the old schedule never plays in the new order or the new one
  in the old ([`retarget.rs`](../src/invention/publish/retarget.rs)).

**Why.** Without declared targets, whether a write lands in this block or
the next depends on module insertion order. Any future module that writes
other modules' controls on the audio thread must declare its targets.

Requests timed on a condition do the same: submitting one orders the
modules it watches before its target where that changes nothing audible, so
the condition is known before the target computes the sample it is met on.

## Reading controls back

Reads return **applied** state, never pending writes. Each declared module
publishes its controls' values to
[`ControlCells`](../src/control_request/cells.rs), one atomic word per
control holding kind and bits together, so a read is never torn. Cells are
independent: two controls read one after the other may straddle an apply.
State that must be read as one across several controls needs a snapshot of
its own.

A payload control is the exception: no cell holds it, so it reads back the
last write the surface accepted.

What is still on its way is kept separately, in the publisher's pending
log, which also records each applied write in the session's request log.

## Intent: author or perform

A control write is either:

- **Author**: the value becomes the invention's new starting state. It is
  recorded in the retained document (so a save reproduces it) and advances
  the document revision.
- **Perform**: a live gesture. It is applied and announced as
  `ControlChanged`, and never recorded.

The wire defaults to `author` for older clients. An event control (a
clock's `reset`, a player's `play`) is never authored, whatever the intent:
it fires and leaves the document as it was, from `SetControl` and from
`ApplyEdits` alike. A config key naming an event (written by hand) never
fires: not as the module first runs, and not on reload, which does not
rebuild the module for it either. That holds for a development instance's
config too, where a key naming an exposed event (one an alias fires) loads
and is ignored. A clock that resets on reload asks for it with its own
`reset_on_reload` key. Conducting scripts and
agents write with `perform`, and scheduler writes are automation that never
reaches the document, so writes at musical rate never make every peer's
edit stale. `ApplyEdits`'s `set_control` is otherwise always authoring.

The choice is made in `RunningInvention::set_control_with_intent`
([`orchestration.rs`](../src/invention/orchestration.rs)). An authored
write and a waiting timed edit are reconciled so that what is recorded is
what plays: the write cancels an edit that would rebuild its module, and is
kept over an edit that writes the same control as it lands.

How revisions and preconditions protect concurrent editors is covered in
[`shared-runtime.md`](shared-runtime.md).

## Front doors

Every write from outside the audio thread goes through one choke point,
`RuntimeSnapshot::submit_control` (behind `set_control_with_intent`, in
[`orchestration.rs`](../src/invention/orchestration.rs)). That covers RPC
`SetControl` and `SetControls`, conducting scripts, agents, and the
offline `RenderEngine` behind FFI and WASM. A write there is:

- coerced to the control's kind, with a number that is not finite refused;
- submitted as a request with its source (user, agent, script) and
  priority;
- announced as `ControlChanged`, naming its request, once it applies.

`RunningInvention::write_controls` tells its caller what became of each
write:

- It waits up to four device blocks, clamped to 20–200 ms, and not at all
  when no device is playing.
- Each write is then reported applied, pending, superseded, overridden or
  refused.
- A pending write is announced if it applies later. Nothing is announced
  for one lost after its reply.
- `ListPendingRequests` lists what is still waiting. By default it is
  compact (the oldest eight, and a count of the rest), and it can list in
  full or by source.

[`runtime/tests/front_doors.rs`](../src/invention/runtime/tests/front_doors.rs)
guards the choke point in two ways:

- Every control of a type that declares its controls is written as a
  request through the front door. A schedule is the exception: it travels
  in a publication. So are a few listed text controls a module reads on a
  control thread.
- Only a listed set of files may write a surface directly, each for a
  stated reason (building a module, validating an edit, carrying values
  into a rebuilt graph).

## Not yet built

These are part of the model but not wired through yet:

- **Intent on the queue.** The request's `intent` field is not yet read:
  intent is applied above the queue.
- **Repeating, ramped and condition-timed control writes on the wire.**
  `SetControl` and `SetControls` take a `when` (a sample, a beat, or
  another request applying) and a `ttl_ms` (see
  [`rpc/control_timing.rs`](../src/rpc/control_timing.rs)), as a
  conducting script's `graph.setControl(id, key, value, { when, ttl_ms })`
  does, which returns the write's request for a later `after_request`; repeating
  grids, beat ramps and signal or control conditions are implemented on the
  audio side only.
- Ramp shapes, and controls declared by plugins.
