//! Requests timed in beats ([`When::Beat`]), waiting on the audio thread
//! for their clock to reach the beat.
//!
//! A beat is resolved as the clock plays, never ahead of it: at each
//! segment start the audio thread asks every watched clock how many
//! samples remain until its beat at the tempo it has now
//! ([`Timeline::samples_until`]) and ends the segment just before that
//! sample, so the next segment starts on it and the request applies there.
//! Every tempo change that lands at a segment start (a request, or
//! automation written before it) is so honoured exactly. One made inside a
//! segment (a scheduler running sample by sample in a feedback group) is
//! seen at the next segment start: the request then applies there, late,
//! and says so ([`Outcome::AppliedLate`]). It never applies early.
//!
//! A request carrying a ramp starts it at its beat, in the graph's ramps
//! (see [`super::ramps`]).

use super::conditions::{limit, WAITING_RESERVE};
use super::hold::Standing;
use super::pending::{expired, Outcome, Outcomes, Refusal};
use super::ramps::{Ramps, RAMP_CAPACITY};
use super::request::{BeatSpec, ControlTarget, Request, RequestId, RequestValue, RtValue, When};
use super::staged::Applying;
use super::table::BeatTable;
use super::timeline::Timeline;

mod cursor;
mod edges;
mod ramp;
mod room;

pub(crate) use cursor::{Cursors, StepClock, CURSOR_CAPACITY};
pub(crate) use edges::{EdgeCursors, EdgeHost, Ports};
pub(crate) use room::RampRoom;

/// What watches need of the graph: its modules' timelines, and applying a
/// request as a sample-timed one is applied.
pub(crate) trait BeatHost {
    fn timeline(&self, module_idx: usize) -> Option<&dyn Timeline>;

    /// Holds the next sample of the clock at `module_idx` at the position
    /// it now predicts: a tempo change applied before that sample takes
    /// effect after it, so the beat a request applies on is still reached
    /// there.
    fn latch(&mut self, module_idx: usize);

    /// The number `target`'s control holds at this sample, automation
    /// written before it included, or `None` for one holding no number:
    /// where a ramp starts.
    fn current(&mut self, target: &ControlTarget) -> Option<f32>;

    /// Whether the latest write to take `target`'s control was one of
    /// `standing`: none since took it, nor freed it for automation's next
    /// event. A ramp runs on only while it does.
    fn kept(&self, target: &ControlTarget, standing: Standing) -> bool;

    /// Whether `a` and `b` reach a control in common: the same, or, on a
    /// development, an inner one both keys reach.
    fn overlaps(&self, a: &ControlTarget, b: &ControlTarget) -> bool;

    /// Finishes the events applied so far, as the pending store's caller
    /// does after a late one (see [`crate::Module::finish_events`]).
    fn finish_events(&mut self) {}

    /// The table the module at `module_idx` plays automation from, if any
    /// (see [`super::table`]).
    fn table(&self, module_idx: usize) -> Option<&dyn BeatTable> {
        let _ = module_idx;
        None
    }

    /// Whether an edit installing at this sample, once what is due here
    /// has applied, keeps the table of the module at `module_idx` playing:
    /// the table then plays after it, on the graph it installs, so an
    /// entry due here, or a ramp's write, reaches a target it rebuilds,
    /// and the tables it keeps still write in module order. An edit a
    /// condition makes due only as this sample's entries write comes after
    /// them:
    /// they play on the graph it replaces.
    fn defers(&self, module_idx: usize) -> bool {
        let _ = module_idx;
        false
    }

    /// Applies `value` as the pending store's `apply` does, of `standing`
    /// (none for an event), weighed against the write standing on its
    /// control at the sample when `outcomes` names it applying (see
    /// [`Outcomes::applying`]): `Err` is what it reports instead.
    fn apply(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        outcomes: &mut Outcomes,
    ) -> Result<(), Outcome>;
}

/// The beat a watch waits for, in its clock's positions.
#[derive(Clone, Copy)]
enum Goal {
    /// [`BeatSpec::After`], until the watch is first passed over.
    After(f64),
    /// [`Self::Count`] set at the start of the pass that first saw it,
    /// before any request of that pass applied: not yet looked at.
    Armed { position: f64, beats_before: u64 },
    /// A position, as of when the clock had begun `beats_before` beats
    /// before its latest reset (see [`Timeline::beats_before`]).
    Count { position: f64, beats_before: u64 },
    /// [`BeatSpec::Grid`], and the beat it waits for next as of
    /// `beats_before`, once looked for.
    Grid {
        every: f64,
        offset: f64,
        repeat: bool,
        next: Option<(f64, u64)>,
    },
}

struct Watch {
    request: Request,
    /// Its clock, in the order of its target's generation.
    clock: usize,
    goal: Goal,
    /// The sample it last applied at, if it repeats (or lost that beat to
    /// a higher rank).
    applied: Option<u64>,
    /// Whether a repeat has settled: at the first beat it applied on.
    settled: bool,
    /// The latest sample a repeat latched its clock on and lost the beat,
    /// reported as it ends if it never applied.
    latched: Option<u64>,
}

impl Watch {
    /// The position it waits for, and whether it was set just now (so a
    /// clock already past it has not missed it).
    fn target(&mut self, timeline: &dyn Timeline, now: u64) -> (f64, bool) {
        let begun = timeline.beats_before();
        match self.goal {
            Goal::After(beats) => {
                let position = timeline.position().max(0.0) + beats;
                self.goal = Goal::Count {
                    position,
                    beats_before: begun,
                };
                (position, true)
            }
            // Each reset since jumps the count to the next whole beat.
            Goal::Count {
                position,
                beats_before,
            } => (position - begun.saturating_sub(beats_before) as f64, false),
            Goal::Armed {
                position,
                beats_before,
            } => {
                self.goal = Goal::Count {
                    position,
                    beats_before,
                };
                (position - begun.saturating_sub(beats_before) as f64, true)
            }
            Goal::Grid {
                next: Some((next, beats_before)),
                ..
            } if beats_before == begun => (next, false),
            // Set again after a reset, from the new beat 0.
            Goal::Grid {
                every,
                offset,
                repeat,
                ..
            } => {
                let from = if self.applied == Some(now) {
                    timeline.position_after(1)
                } else {
                    timeline.position()
                };
                let next = grid_after(from, every, offset);
                self.goal = Goal::Grid {
                    every,
                    offset,
                    repeat,
                    next: Some((next, begun)),
                };
                (next, true)
            }
        }
    }

    fn repeats(&self) -> bool {
        matches!(self.goal, Goal::Grid { repeat: true, .. })
    }
}

/// The least `offset + m * every` past `from`.
fn grid_after(from: f64, every: f64, offset: f64) -> f64 {
    let offset = offset.rem_euclid(every);
    let mut m = ((from - offset) / every).floor() + 1.0;
    if offset + (m - 1.0) * every > from {
        m -= 1.0;
    }
    if offset + m * every <= from {
        m += 1.0;
    }
    offset + m * every
}

/// The sample a clock now past `target` reached it at, estimated at its
/// current tempo: before `now`, unless a reset at `now` passed it (the
/// clock has not started since), which is on time.
fn missed_at(timeline: &dyn Timeline, target: f64, now: u64) -> u64 {
    if timeline.position() < 0.0 {
        return now;
    }
    let rate = timeline.position_after(1) - timeline.position();
    let past = if rate > 0.0 && rate.is_finite() {
        ((timeline.position() - target) / rate).floor() as u64
    } else {
        0
    };
    now.saturating_sub(past.saturating_add(1))
}

/// The finest repeating grid, in beats (a 256th note): each application
/// ends a segment, so a finer one would split every block into a few
/// samples for as long as it runs.
pub(crate) const MIN_REPEAT_STEP: f32 = 1.0 / 64.0;

/// Requests waiting for a beat, in receipt order, and apart from them the
/// ramps they start. Single-threaded (the audio thread), allocated once on
/// a control thread and never grown.
pub(crate) struct Watches {
    entries: Vec<Watch>,
    /// The watches it holds at most (the Vec may have more room).
    limit: usize,
    /// Ramps running, in a table of their own (see [`Ramps`]).
    pub(crate) ramps: Ramps,
    /// Tables played in place (see [`cursor`]).
    pub(crate) cursors: Cursors,
    /// Tables played on their modules' edges (see [`edges`]).
    pub(crate) edges: EdgeCursors,
}

impl Watches {
    /// Room for `capacity` watches, and [`WAITING_RESERVE`] more reserved
    /// ones. Allocates: control thread.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity + WAITING_RESERVE),
            limit: capacity,
            ramps: Ramps::new(RAMP_CAPACITY),
            cursors: Cursors::new(CURSOR_CAPACITY),
            edges: EdgeCursors::new(CURSOR_CAPACITY),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no watch waits, no ramp runs and no table plays.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.ramps.is_empty() && self.cursors.is_empty()
    }

    /// Whether a watch, `reserved` or not, finds no room.
    pub(crate) fn is_full(&self, reserved: bool) -> bool {
        self.entries.len() >= limit(self.limit, reserved)
    }

    /// Refuses a beat no clock can reach (a span or grid that is not
    /// finite, a negative span, a grid step that is not positive), a
    /// repeating grid finer than [`MIN_REPEAT_STEP`], and a repeating
    /// payload, which applies once. A ramp must be finite and positive and
    /// write a finite number, once: one on an event, a payload, another
    /// value or a repeating grid is refused.
    pub(crate) fn check(request: &Request) -> Result<(), Refusal> {
        let When::Beat(beat) = request.when else {
            return Err(Refusal::Unsupported);
        };
        if let Some(beats) = beat.ramp {
            let repeats = matches!(beat.spec, BeatSpec::Grid { repeat: true, .. });
            match request.value {
                _ if request.event || repeats => return Err(Refusal::Unsupported),
                RequestValue::Value(RtValue::F32(to))
                    if to.is_finite() && beats.is_finite() && beats > 0.0 => {}
                RequestValue::Value(RtValue::F32(_)) => return Err(Refusal::Invalid),
                _ => return Err(Refusal::Unsupported),
            }
        }
        match beat.spec {
            BeatSpec::After(beats) if beats.is_finite() && beats >= 0.0 => Ok(()),
            BeatSpec::Grid { every, offset, .. }
                if !(every.is_finite() && every > 0.0 && offset.is_finite()) =>
            {
                Err(Refusal::Invalid)
            }
            BeatSpec::Grid {
                every,
                repeat: true,
                ..
            } if every < MIN_REPEAT_STEP => Err(Refusal::Invalid),
            BeatSpec::Grid { repeat: true, .. } if request.value.is_payload() => {
                Err(Refusal::Unsupported)
            }
            BeatSpec::Grid { .. } => Ok(()),
            BeatSpec::After(_) => Err(Refusal::Invalid),
        }
    }

    /// Stores `request`, timed on `clock` (both in its target's
    /// generation), or settles it refused ([`Refusal::PendingFull`]) when
    /// full, or when it carries a ramp the ramps have no room for.
    /// [`Self::check`] has passed it. Allocation- and free-free.
    pub(crate) fn insert(&mut self, request: Request, clock: usize, outcomes: &mut Outcomes) {
        let When::Beat(beat) = request.when else {
            return outcomes.settle(request, Outcome::Refused(Refusal::Unsupported));
        };
        let no_ramp_room = beat.ramp.is_some() && !self.ramps.has_room(self.ramps_waiting());
        if self.is_full(request.reserved) || no_ramp_room {
            return outcomes.settle(request, Outcome::Refused(Refusal::PendingFull));
        }
        let goal = match beat.spec {
            BeatSpec::After(beats) => Goal::After(f64::from(beats)),
            BeatSpec::Grid {
                every,
                offset,
                repeat,
            } => Goal::Grid {
                every: f64::from(every),
                offset: f64::from(offset),
                repeat,
                next: None,
            },
        };
        self.entries.push(Watch {
            request,
            clock,
            goal,
            applied: None,
            settled: false,
            latched: None,
        });
    }

    /// The clocks of the watches, ramps and tables in the `installed`
    /// generation.
    pub(crate) fn clocks(&self, installed: u64) -> impl Iterator<Item = usize> + '_ {
        self.entries
            .iter()
            .filter(move |watch| watch.request.target.generation == installed)
            .map(|watch| watch.clock)
            .chain(self.ramps.clocks(installed))
            .chain(self.cursors.clocks())
    }

    /// Maps every watch resolved against a generation older than
    /// `installed` into it, its target and its clock alike: `map` gives a
    /// module's index in the installed order, or `None` when it went away,
    /// which settles the watch refused ([`Refusal::TargetGone`], or
    /// [`Refusal::TimelineGone`] for its clock). Allocation- and free-free.
    pub(crate) fn remap(
        &mut self,
        installed: u64,
        outcomes: &mut Outcomes,
        mut map: impl FnMut(u64, usize) -> Option<usize>,
    ) {
        let mut i = 0;
        while i < self.entries.len() {
            let watch = &mut self.entries[i];
            let generation = watch.request.target.generation;
            if generation >= installed {
                i += 1;
                continue;
            }
            let module = map(generation, watch.request.target.module_idx);
            let clock = map(generation, watch.clock);
            let refusal = match (module, clock) {
                (Some(module), Some(clock)) => {
                    watch.request.target.generation = installed;
                    watch.request.target.module_idx = module;
                    watch.clock = clock;
                    i += 1;
                    continue;
                }
                (None, _) => Refusal::TargetGone,
                (_, None) => Refusal::TimelineGone,
            };
            self.end(i, outcomes, Outcome::Refused(refusal));
        }
        // A ramp maps with its target and clock: one gone ends it.
        self.ramps.remap(installed, map);
    }

    /// Removes watch `i`, settling it with `outcome`, unless it repeats and
    /// has applied: it settled then, at its first application, and ends
    /// silently. Allocation- and free-free.
    fn end(&mut self, i: usize, outcomes: &mut Outcomes, outcome: Outcome) {
        let watch = self.entries.remove(i);
        if !watch.settled {
            outcomes.settle_on_beat(watch.request, outcome, watch.latched);
        }
    }

    /// Starts every new span in the `installed` generation counting from
    /// its clock's position now. Called before anything at this sample can
    /// move a clock (a reset, timed in samples or in beats), so a span
    /// counts from where the clock was when the audio thread took it,
    /// whatever applies with it. Allocation- and free-free.
    pub(crate) fn arm(&mut self, installed: u64, host: &impl BeatHost) {
        for watch in &mut self.entries {
            let installed = watch.request.target.generation == installed;
            if let (true, Goal::After(beats)) = (installed, watch.goal) {
                if let Some(timeline) = host.timeline(watch.clock) {
                    watch.goal = Goal::Armed {
                        position: timeline.position().max(0.0) + beats,
                        beats_before: timeline.beats_before(),
                    };
                }
            }
        }
    }

    /// One pass at sample `now` over the watches in the `installed`
    /// generation: applies each whose clock's next sample reaches its beat
    /// (or, late, has passed it), and refuses each whose ttl ran out or
    /// whose clock has no timeline. Returns whether any applied, which may
    /// have moved a clock (a reset or tempo timed on beats), so the caller
    /// passes again until none does, and how many samples until the next
    /// may, as far as the clocks' tempi now say. A repeating watch applies
    /// at most once per sample, and settles at the first beat it applies on
    /// only; one losing every beat to a higher rank settles as it ends
    /// (refused [`Refusal::Expired`] at its ttl, say). Then it fires every
    /// table entry due (see [`cursor`]). Allocation- and free-free.
    pub(crate) fn pass(
        &mut self,
        now: u64,
        installed: u64,
        host: &mut impl BeatHost,
        outcomes: &mut Outcomes,
    ) -> (bool, Option<u64>) {
        self.arm(installed, host);
        let (mut applied, mut wait) = (false, None::<u64>);
        let mut i = 0;
        while i < self.entries.len() {
            let watch = &mut self.entries[i];
            if watch.request.target.generation != installed {
                i += 1;
                continue;
            }
            if expired(&watch.request, now) {
                self.end(i, outcomes, Outcome::Refused(Refusal::Expired));
                continue;
            }
            let Some(timeline) = host.timeline(watch.clock) else {
                self.end(i, outcomes, Outcome::Refused(Refusal::NoTimeline));
                continue;
            };
            let (beat, fresh) = watch.target(timeline, now);
            if beat.is_nan() || timeline.position().is_nan() {
                // A clock whose position is no number reaches no beat.
                self.end(i, outcomes, Outcome::Refused(Refusal::Invalid));
                continue;
            }
            // A repeat applies once a sample at most, however its grid's
            // arithmetic rounds (a position past f64's integers stands
            // still), so the passes end.
            let again = watch.applied == Some(now);
            let due = if again {
                None
            } else if !fresh && timeline.position() >= beat {
                Some(missed_at(timeline, beat, now))
            } else if timeline.position_after(1) >= beat {
                Some(now)
            } else {
                None
            };
            let Some(due) = due else {
                if let Some(samples) = timeline.samples_until(beat) {
                    // Up to the sample before it reaches the beat; a
                    // repeat that applied now waits a sample at least.
                    let samples = (samples - 1).max(1);
                    wait = Some(wait.map_or(samples, |wait| wait.min(samples)));
                }
                i += 1;
                continue;
            };
            applied = true;
            let outcome = |result: Result<(), Outcome>| match result {
                Ok(()) if due < now => Outcome::AppliedLate { at: now, due },
                Ok(()) => Outcome::Applied { at: now },
                Err(outcome) => outcome,
            };
            if due == now {
                host.latch(watch.clock);
            }
            if matches!(watch.request.when, When::Beat(time) if time.ramp.is_some()) {
                self.start_ramp(i, (beat, due, now), host, outcomes);
                continue;
            }
            let standing = Standing::of(&watch.request, due);
            let target = watch.request.target;
            // A late event finishes as it applies, as the pending store's.
            let late_event = watch.request.fires() && due < now;
            if let (true, RequestValue::Value(value)) = (watch.repeats(), &watch.request.value) {
                let value = RequestValue::Value(*value);
                outcomes.applying(on_beat(watch.request.id, due, now));
                let result = host.apply(&watch.request.target, standing, value, outcomes);
                if result.is_err() && due == now {
                    self.entries[i].latched = Some(now);
                }
                // A beat lost to a higher rank at its sample is only that
                // beat: the next is the grid's next event, and it runs on.
                if let Err(Outcome::Refused(_)) = result {
                    self.end(i, outcomes, outcome(result));
                    continue;
                }
                if result.is_ok() {
                    self.ramps.supersede(&target, &*host);
                    if late_event {
                        host.finish_events();
                    }
                }
                let watch = &mut self.entries[i];
                // Settled at the first beat it applied on only: the
                // outcome queue holds one per request, and a repeat runs
                // on.
                if result.is_ok() && !watch.settled {
                    outcomes.record(watch.request.id, false, outcome(result), true);
                    watch.settled = true;
                }
                watch.applied = Some(now);
                if let Goal::Grid { next, .. } = &mut watch.goal {
                    *next = None;
                }
                i += 1;
                continue;
            }
            let Request {
                target, value, id, ..
            } = self.entries.remove(i).request;
            let payload = value.is_payload();
            outcomes.applying(on_beat(id, due, now));
            let result = host.apply(&target, standing, value, outcomes);
            if result.is_ok() {
                self.ramps.supersede(&target, &*host);
                if late_event {
                    host.finish_events();
                }
            }
            // Refused or overridden on its beat, it still latched there.
            let latched = (due == now && result.is_err()).then_some(now);
            outcomes.record_on_beat(id, payload, outcome(result), latched, true);
        }
        // Then the tables, as a scheduler wrote after the requests due at
        // its sample: at the same tier, the table's entry wins.
        let (fired, next) = self.cursors.pass(now, host, &mut self.ramps);
        (applied || fired, wait.into_iter().chain(next).min())
    }
}

/// Request `id`, due on its beat at `due`, applying at `now`.
fn on_beat(id: RequestId, due: u64, now: u64) -> Applying {
    Applying {
        id,
        timed: true,
        at: now,
        due,
    }
}

#[cfg(test)]
mod tests;
