# Musical time

How Fugue decides *when* something happens: the engine's sample count, the
conditions a request can wait for, and the beats a clock keeps. It also draws
the line between what a score records and what an invention decides it means,
because that line settles where tempo lives.

This document is for contributors and reviewers changing the timing code, and
for composers writing inventions that schedule changes in time. How a timed
write is prioritised, held and recorded is in
[the control model](control-model.md); this document covers only the *when*.

## Three layers

Timing is built in three layers, each on the one below:

1. **The sample transport.** The engine counts samples. It knows nothing of
   beats or tempo.
2. **Conditions.** A request waits until something happens in the running
   invention: a signal edge, a control crossing a level, another request
   applying.
3. **Clock timelines.** A clock keeps a beat position. A request timed in
   beats is a condition on that position, common enough to get its own form.

The split keeps the engine honest. The audio thread only ever answers "which
sample?", so beats, tempo changes and resets are all resolved the same way:
by asking the clock how far away the beat is now, never by precomputing a
sample that a later tempo change would make wrong.

### The sample transport

The engine counts samples from the moment its graph is created
([`transport.rs`](../src/control_request/transport.rs)). The count never goes
back. There is no transport start or stop: an invention plays from load, and
if a piece needs "stop" or "from the top" it builds that from a clock and its
controls.

A request can be timed now, at a sample, some samples from submission, or at
a wall-clock time ([`When`](../src/control_request/request.rs)). A wall-clock
time is converted to the sample a listener hears then, from the device's
callback timing and reported output latency; the error bound is documented on
`Transport::sample_at`. Offline render runs the same counter with no wall
clock, so a request timed in samples lands on the same sample offline as live,
and one timed by wall clock is refused there (`NoClock`).

The audio thread splits each block into segments at the samples requests are
due on, and applies requests between them. A request whose sample has already
passed when the audio thread takes it still applies, at the next segment
start, and reports `AppliedLate` with the sample
it was due. Nothing applies early.

### Clock timelines

A `clock` ([`modules/clock`](../src/modules/clock/mod.rs)) keeps a continuous
beat position, readable as its `position` control:

- **Beat 0 is the clock's first gate.** Beat `N` falls on the first sample
  whose position reaches `N`, which is the sample the `beat` gate rises on. A
  request timed on a beat lands with the beat a listener hears.
- **Tempo changes keep the position continuous.** A `bpm` change re-anchors
  the count where it stands, so no pulse is clipped or doubled at the seam.
  A tempo change timed on a beat takes effect from the sample after, so it
  never moves the beat it lands on.
- **`reset` returns the clock to beat 0.** Its next sample is position 0
  exactly, so resetting every `N` beats loops them `N` beats long, to the
  sample.
- **A reload keeps the position** of a clock it keeps, so an edit lands
  without a jump in the pulse. Set the clock's `reset_on_reload` config to
  `true` to reset it on every reload instead. A `bpm` changed in the reloaded
  document lands as a control write and keeps the position too. An edit batch
  (`ApplyEdits`) always keeps the position, and `reset_on_reload` does not
  reach a clock inside a development.

There is no metre in the engine. A clock knows beats, not bars or time
signatures; bars are the composer's. The clock's outputs are the beat and
fixed multiples of it: `beat`, `beat_d4` (every 4 beats), `beat_d2` (every
2), `beat_x2` (2 per beat) and `beat_x4` (4 per beat).

## Timing a request in beats

A request timed in beats names a clock and one of two shapes
([`BeatSpec`](../src/control_request/request.rs)):

- **After**: this many beats after the clock's position when the audio thread
  takes the request. Counted in beats begun, so a reset while it waits counts
  as reaching the next whole beat, never as going back.
- **Grid**: the next beat congruent to `offset` modulo `every`, counted from
  the clock's beat 0. `every: 4` is "the next downbeat" if the composer counts
  in fours. After a reset the grid follows the new beat 0. With `repeat`, the
  value is applied again at every grid beat until its ttl runs out or its
  target or clock goes away. A repeating grid finer than 1/64 beat is refused,
  since each application splits the audio block.

A request writing a number may also carry a **ramp**: from the control's value
at the beat, it glides linearly to the new value over that many beats of the
clock, arriving exactly on the sample the clock reaches the end.

Beats are resolved as the clock plays, never ahead of it
([`watch.rs`](../src/control_request/watch.rs)). At every segment start the
audio thread asks each watched clock how many samples remain until the beat at
its current tempo, and splits the block there. A tempo change made at a
segment start is honoured exactly; one made in the middle of a segment (a
scheduler in a feedback loop) is seen at the next segment start, and the
request then applies late and says so.

On the wire, timed structural edits (`SwapModule`, `ApplyEdits`,
`ReloadInvention`) accept `when` as `at_sample`, `after_samples`,
`after_beats`, `next_beat` (`every`, `offset`) or `next_bar`
([`rpc/timed.rs`](../src/rpc/timed.rs)). `next_bar` is only a grid of
`beats_per_bar` beats (default 4): a convenience for the caller, not a metre
the engine keeps. The beat forms take an optional `clock`, and every form an
optional `ttl_ms`.

Control writes (`SetControl`, each write of `SetControls`) take the same
`when` and `ttl_ms`, and one more form, `after_request` (`request`,
`offset_samples`): that many samples after another request applies, a
control write's or a timed edit's, named as its reply named it
([`rpc/control_timing.rs`](../src/rpc/control_timing.rs)). The reply names
where the write lands (its sample, or its beat), the pending listing shows
it while it waits, and a `ControlRequestSettled` event reports what became
of it: applied, superseded, overridden, refused or expired. An authored
write is recorded in the document as it applies, and whichever of it and
any other write to the control applies later is what plays and what is
recorded.

### Which clock: the timeline context

A request may name its clock. When it does not, it counts on the clock that
is actually driving the module it targets: the **nearest clock upstream in
signal flow** ([`timelines.rs`](../src/invention/timelines.rs)). The
alternative, the clock that happens to sit above the module in the invention's
nesting, would often name a clock the module never hears; signal flow names
the one whose gates it is following.

The rules, worked out on a control thread each time the topology is compiled:

- Walk back through connections from any of the module's inputs; each path
  ends at the first clock on it. The nearest clock, counted in connections,
  wins. A clock's timeline is its own.
- Developments are crossed as if laid out flat.
- A clock inside a development cannot be named by a request yet. If it is the
  nearest, an unnamed request is refused rather than silently counting on a
  clock the module does not follow.
- Ties go to a clock a request can name, then to the one first in module
  order.
- With no clock connected, the context falls back to the first top-level clock
  in module order. If no clock can be named (none at all, or only clocks
  inside developments), an unnamed request timed in beats is refused.

A structural edit that targets one module (a swap) uses that module's context.
An edit batch or a reload has no single module, so without a named clock it
counts on the invention's first top-level clock in module order.

The clock is resolved once, when the request is submitted. Later graph changes
recompute contexts, but a waiting request keeps the clock it resolved
against, and is refused `TimelineGone` if that clock is removed or rebuilt.

## Conditions

A condition is data, not code
([`condition.rs`](../src/control_request/condition.rs)): up to four parts,
all of which must have happened. Each part is an event that is met at one
sample and stays met; the condition is met at the sample its last part is,
and the request applies there. The kinds:

| Part | Met when | Accuracy |
| --- | --- | --- |
| Signal | an output crosses a level, rising or falling, `count` times | The exact sample |
| Control | a control crosses a level, rising or falling | The first segment start that sees it |
| Applied | another request applies | The sample it applies |

Counting a gate's notes, a sequencer's steps or a clock's beats is a signal
part on rising edges past 0.5; a threshold on an output is a count of one.

A signal part is exact only if the module it watches computes before the
module the request targets. When a condition is submitted, the publisher adds
an ordering-only edge to make that so where it can. Where it cannot (the
target already feeds the watched module, or reordering would change what is
heard), the request applies at the target's next segment and reports
`AppliedLate`
([`publish/watch.rs`](../src/invention/publish/watch.rs)). An `Applied` part
whose request settles without applying refuses the waiting one
(`ChainBroken`): this is how chained edits fail cleanly.

Parts combine with AND only. The `Join` and `Predicate` enums are
non-exhaustive so that OR, NOT, sequences and new predicate kinds can be added
without changing how the audio thread drains conditions.

### Expiry and refusal

Nothing waits forever by accident. Any request may carry a **ttl**: past it,
the request is refused `Expired` instead of applying. A request also settles,
refused, when what it waits on goes away: its target (`TargetGone`), its clock
(`TimelineGone`) or a module a condition reads (`SourceGone`). A beat on a
clock whose tempo is not positive is never reached, so give such requests a
ttl.

### Limits

The audio thread's tables are allocated once, on a control thread, and never
grow. Per graph (the constants are in
[`src/control_request/`](../src/control_request/), except
`BEAT_WATCH_CAPACITY` in
[`publisher.rs`](../src/invention/publish/publisher.rs)):

| Table | Capacity | Past it |
| --- | --- | --- |
| Requests waiting for a beat | 64 (`BEAT_WATCH_CAPACITY`) | `PendingFull` |
| Requests waiting on a condition | 64 (`CONDITION_CAPACITY`) | refused before queuing |
| Parts per condition | 4 (`MAX_PARTS`) | refused before queuing |
| Beat ramps running | 64 (`RAMP_CAPACITY`) | `PendingFull` |
| Schedule-table ramps running | 64 | the value is written at once, and counted |
| Schedules the drain plays on clocks | 32 (`CURSOR_CAPACITY`) | the scheduler plays its own |

## Score and interpretation

A score (`fugue.score.v1`, [`score.rs`](../src/invention/score.rs)) records
what is notated: notes, dynamics as per-step `velocity`, the tempo map, pedal
spans, grace notes, time signature. The invention decides what those mean:
how loud a `velocity` sounds, how a pedal span decays, how long a grace note
lasts and where it steals time from, how fast the clock runs. Performers,
human or agent, bend the interpretation live; they never edit the notation to
do it.

This keeps one score playable by many inventions, and keeps an import honest:
a score importer only has to get the notation right, and its accuracy can be
checked against the source.

Score-derived facts that change over time compile into
[`control_scheduler`](../src/modules/control_scheduler/mod.rs) schedules rather
than being baked into module configs. A tempo map spliced into a scheduler's
`tempo_map` becomes one entry per change, writing the clock's `bpm` at that
step, with `ramp_steps` for a ritardando or accelerando.

### The clock carries the notated tempo

The clock runs at the tempo the score notates, in quarter-note beats. How
finely the sequencers step is a patching choice, not a tempo scale.

Take a Bach Invention written in sixteenth notes and played at ♩ = 72. Set the
clock's `bpm` to 72 and patch the sequencers' `clock` inputs from `beat_x4`, so
each step is a sixteenth. Patch the tempo scheduler from the same `beat_x4`,
so its steps line up with the score's: the score's tempo map then compiles one
for one into the clock's schedule, with no scale factor.

The alternative, patching the sequencers from `beat` and running the clock at
four times the notated tempo, works for a while and then misleads. The clock's
`bpm` no longer reads as the piece's tempo, a tempo map needs a hidden ×4 to
compile, and fast passages push `bpm` far past the 1 to 300 range editors
offer.

The scheduler's `tempo_scale` config (default `1.0`) multiplies every compiled
tempo. Use it only when an invention genuinely reinterprets the tempo (a
deliberately slower reading, say), never to make up for subdivision.

## Beat-synced gates and fractional steps

At most tempos a step is not a whole number of samples: sixteenths at 97 BPM
on 48 kHz are 7,422.68 samples each. A sequencer measures its step length
from the gap between clock edges, so the length it measures can be a sample
longer than the next step turns out to be.

A held note followed by a new note must therefore **release with a real gap**
before the new note's edge, not fill the whole measured step and rely on the
gate dipping for one sample at the boundary. That dip only happens when steps
are a whole number of samples; at a fractional step the gate can stay high
across the edge, so the next note never retriggers and onsets drop out
irregularly.

The [`cell_sequencer`](../src/modules/cell_sequencer/mod.rs) does this: a held
step releases at its `gate_length` when a new note follows, and still sustains
fully into a rest, the end of the cell, or another held step. Both it and the
[`step_sequencer`](../src/modules/step_sequencer/mod.rs) also force a
one-sample release when a new note arrives while the gate is still high (the
first step, before a step length is measured, or a sudden accelerando), as a
backstop. The `step_sequencer` still holds a held step's gate for the full
step into a new note, and so relies on that backstop.

The rule generalises: any module that gates or retriggers on musical steps
must not assume a step is a whole number of samples.

## Not built yet

- Naming a clock inside a development from a request.
- OR, NOT and sequence joins for conditions.
- Repeating grids, beat ramps and signal or control conditions on the
  wire: they exist in the engine and its in-process API, but a control
  write over RPC takes a single sample, beat or request to follow.
- Metre awareness. If it comes, it belongs in a module that adds bars to the
  timeline context, not in the engine's beat count.
