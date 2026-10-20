//! Tables played in place: one more kind of watch, a cursor over a
//! module's [`BeatTable`], firing each entry at the sample its clock
//! reaches the entry's step, as a request timed in beats applies.
//!
//! # Steps
//!
//! A table's step is one gate of the clock output connected to its clock
//! input: a beat, or the output's division of one, on the clock's beat
//! grid whatever its gates' length (one at 0 or 1, never rising, still
//! steps it, as the `step` output follows the clock). Step 0 falls on the
//! clock's first gate past where it stood when the drain first saw the
//! table (on beat 0 for a clock not yet started), and again on beat 0
//! after every reset of the clock, which ends the table's ramps then
//! running: the table plays again from its start. Another plan plays on
//! from the step the clock is in: entries at or before it have had their
//! gate, and the old plan's ramps end.
//!
//! # Writes
//!
//! Each entry is automation's next event on its control (see
//! [`crate::control_request::TableTarget`]): it writes, or starts a ramp
//! in [`Ramps`] replacing the table's ramp there; with no room, it writes.

use super::{grid_after, missed_at, BeatHost};
use crate::control_request::hold::FrameScope;
use crate::control_request::ramps::{BeatRamp, EntryKey, Ramps};
use crate::control_request::table::{BeatTable, TableValue};
use crate::control_request::timeline::Timeline;

/// The most tables one graph's drain plays: a module whose table finds no
/// room plays it itself.
pub(crate) const CURSOR_CAPACITY: usize = 32;

/// Where a table's step 0 falls.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Anchor {
    /// In its clock's positions, as of when the clock had begun
    /// `beats_before` beats before its latest reset.
    origin: f64,
    beats_before: u64,
}

/// The clock a table steps on, as the graph finds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StepClock {
    /// Its module, in the installed order.
    pub(crate) module: usize,
    /// Tells it from every other clock module, whatever its index.
    pub(crate) id: u64,
    /// Beats per gate of the output driving the table.
    pub(crate) beats: f64,
}

struct Cursor {
    /// The table's id ([`BeatTable::id`]).
    table: usize,
    /// Its module, in the installed order.
    module: usize,
    clock: StepClock,
    /// Set once the clock is first looked at.
    anchor: Option<Anchor>,
    /// The plan it plays, and the index of its next entry.
    plan: u64,
    next: usize,
    /// Found again since the graph last looked for tables.
    found: bool,
}

impl Cursor {
    /// The position of step 0, setting it on first sight and again after
    /// a reset of the clock, and following another plan: `table`'s
    /// ramps end when it starts over or changes plan.
    fn anchor(&mut self, timeline: &dyn Timeline, table: &dyn BeatTable, ramps: &mut Ramps) -> f64 {
        let begun = timeline.beats_before();
        let anchor = match self.anchor {
            Some(anchor) if anchor.beats_before == begun => anchor,
            seen => {
                // After a reset, step 0 is the clock's new beat 0.
                let origin = match seen {
                    Some(_) => {
                        ramps.end_table(self.table);
                        0.0
                    }
                    None => grid_after(timeline.position(), self.clock.beats, 0.0),
                };
                self.next = 0;
                self.plan = table.plan();
                *self.anchor.insert(Anchor {
                    origin,
                    beats_before: begun,
                })
            }
        };
        if table.plan() != self.plan {
            ramps.end_table(self.table);
            self.plan = table.plan();
            let reached = timeline.position();
            let step = |index| {
                let entry = table.entry(index)?;
                Some(anchor.origin + entry.step as f64 * self.clock.beats)
            };
            self.next = 0;
            while step(self.next).is_some_and(|beat| beat <= reached) {
                self.next += 1;
            }
        }
        anchor.origin
    }
}

/// The tables a graph's drain plays, one cursor each. Single-threaded
/// (the audio thread), allocated once on a control thread and never
/// grown.
pub(crate) struct Cursors {
    entries: Vec<Cursor>,
    limit: usize,
}

impl Cursors {
    /// Room for `capacity` tables. Allocates: control thread.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            limit: capacity,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The clocks the tables step on.
    pub(crate) fn clocks(&self) -> impl Iterator<Item = usize> + '_ {
        self.entries.iter().map(|cursor| cursor.clock.module)
    }

    /// Whether it plays table `table`.
    pub(crate) fn has(&self, table: usize) -> bool {
        self.entries.iter().any(|cursor| cursor.table == table)
    }

    /// Starts looking for the graph's tables again, after the modules
    /// changed: those not found by [`Self::sought`] end there.
    pub(crate) fn seek(&mut self) {
        for cursor in &mut self.entries {
            cursor.found = false;
        }
    }

    /// Table `table` is at `module`, stepping on `clock`, or on no clock it
    /// can play (none connected, or a signal other than a clock's gate).
    /// Returns whether the drain plays it: on a clock, with room. One whose
    /// clock changed starts over, as on first sight. Allocation- and
    /// free-free.
    pub(crate) fn found(
        &mut self,
        table: usize,
        module: usize,
        clock: Option<StepClock>,
        ramps: &mut Ramps,
    ) -> bool {
        let at = self.entries.iter().position(|cursor| cursor.table == table);
        let clock = clock.filter(|clock| clock.beats.is_finite() && clock.beats > 0.0);
        let (Some(clock), at) = (clock, at) else {
            return false;
        };
        let Some(i) = at else {
            if self.entries.len() >= self.limit {
                return false;
            }
            self.entries.push(Cursor {
                table,
                module,
                clock,
                anchor: None,
                plan: 0,
                next: 0,
                found: true,
            });
            return true;
        };
        let cursor = &mut self.entries[i];
        let same = (cursor.clock.id, cursor.clock.beats) == (clock.id, clock.beats);
        if !same {
            ramps.end_table(table);
            cursor.anchor = None;
        }
        cursor.module = module;
        cursor.clock = clock;
        cursor.found = true;
        ramps.retable(table, module, clock.module);
        true
    }

    /// Ends the cursors of tables not found since [`Self::seek`], and
    /// their ramps. Allocation- and free-free.
    pub(crate) fn sought(&mut self, ramps: &mut Ramps) {
        self.entries.retain(|cursor| {
            if !cursor.found {
                ramps.end_table(cursor.table);
            }
            cursor.found
        });
    }

    /// Puts the cursors in their modules' order, so entries of several
    /// tables firing at one sample write in module order (the last
    /// module's write lands), however and whenever each table was found.
    /// In place: allocation- and free-free.
    pub(crate) fn order(&mut self) {
        self.entries.sort_unstable_by_key(|cursor| cursor.module);
    }

    /// One pass at sample `now`: tells each table its step and fires every
    /// entry whose step the clock's next sample reaches (or, late, has
    /// passed). Returns whether any fired, and how many samples until the
    /// next entry or step may, as far as the clocks' tempi now say.
    /// Allocation- and free-free.
    pub(crate) fn pass(
        &mut self,
        now: u64,
        host: &mut impl BeatHost,
        ramps: &mut Ramps,
    ) -> (bool, Option<u64>) {
        let (mut any, mut wait) = (false, None::<u64>);
        for cursor in &mut self.entries {
            let fired = fire(cursor, now, &*host, ramps);
            // The next sample stays where the clock now predicts, so an
            // entry writing its tempo lands its following steps there.
            if fired.on_time {
                host.latch(cursor.clock.module);
            }
            any |= fired.any;
            wait = wait.into_iter().chain(fired.wait).min();
        }
        (any, wait)
    }
}

/// What a cursor's pass did: whether it fired any entry, whether one was
/// due at this sample (latching its clock), and the samples until its
/// next entry or step may be: `None` once its clock or table is gone.
#[derive(Default)]
struct Fired {
    any: bool,
    on_time: bool,
    wait: Option<u64>,
}

/// Fires `cursor`'s entries due at `now`.
fn fire(cursor: &mut Cursor, now: u64, host: &impl BeatHost, ramps: &mut Ramps) -> Fired {
    let mut fired = Fired::default();
    let Some(timeline) = host.timeline(cursor.clock.module) else {
        return fired;
    };
    let table = host.table(cursor.module);
    let Some(table) = table.filter(|table| table.id() == cursor.table) else {
        return fired;
    };
    if timeline.position().is_nan() {
        return fired;
    }
    let origin = cursor.anchor(timeline, table, ramps);
    let next_step = report_step(table, timeline, origin, cursor.clock.beats);
    let until = |beat| {
        timeline
            .samples_until(beat)
            .map(|samples| (samples - 1).max(1))
    };
    fired.wait = next_step.and_then(until);
    while let Some(entry) = table.entry(cursor.next) {
        let beat = origin + entry.step as f64 * cursor.clock.beats;
        let due = if timeline.position() >= beat {
            missed_at(timeline, beat, now)
        } else if timeline.position_after(1) >= beat {
            now
        } else {
            fired.wait = fired.wait.into_iter().chain(until(beat)).min();
            return fired;
        };
        fired.any = true;
        fired.on_time |= due == now;
        let key = EntryKey {
            module: cursor.module,
            table: cursor.table,
            plan: cursor.plan,
            entry: cursor.next,
        };
        cursor.next += 1;
        // Weighed against a higher tier's write as of its own sample. The
        // ramp it replaces writes first, so a hold still drops that write.
        let _scope = FrameScope::enter(due);
        ramps.supersede_entry(&entry.target, host);
        entry.target.begin(0);
        let ramp = match (entry.ramp_steps, entry.value) {
            (0, _) | (_, TableValue::Bool(_)) => None,
            // A ramp leaves the control where it stands: one holding no
            // number is skipped, as a scheduler always skipped it.
            (steps, TableValue::Number(to)) => {
                let Some(from) = entry.target.current() else {
                    continue;
                };
                let beats = steps as f64 * cursor.clock.beats;
                Some(BeatRamp::new(
                    from,
                    to,
                    beat,
                    beats,
                    timeline.beats_before(),
                ))
            }
        };
        let clock = cursor.clock.module;
        // With no ramp room the entry jumps to its value: `start_entry`
        // counts it (`Ramps::crowded`).
        if !ramp.is_some_and(|ramp| ramps.start_entry(key, clock, ramp, host)) {
            entry.target.write(entry.value);
        }
    }
    fired
}

/// Beyond this a step no longer counts exactly in a beat (an `f64`): a
/// clock this far along tells the last step it can.
const LAST_STEP: i64 = 1 << 53;

/// Tells `table` the step its clock's next sample is in, counting steps
/// of `beats` from `origin` as its entries count them. Returns the beat
/// the next step falls on, or `None` at the last step it can count, so a
/// clock that far along does not end every segment after one sample.
fn report_step(
    table: &dyn BeatTable,
    timeline: &dyn Timeline,
    origin: f64,
    beats: f64,
) -> Option<f64> {
    let reached = timeline.position_after(1);
    let at = |step: i64| origin + step as f64 * beats;
    let steps = ((reached - origin) / beats).floor();
    let mut step = if steps.is_nan() {
        -1
    } else {
        steps.clamp(-1.0, LAST_STEP as f64) as i64
    };
    // The division may round across a boundary its entries' beats do not,
    // by a step at most.
    if step >= 0 && at(step) > reached {
        step -= 1;
    } else if step < LAST_STEP && at(step + 1) <= reached {
        step += 1;
    }
    table.set_step(step);
    (step < LAST_STEP).then(|| at(step + 1))
}
