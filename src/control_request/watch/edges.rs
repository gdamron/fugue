//! Tables played on rising edges: a cursor over a module's [`BeatTable`]
//! whose clock input carries something other than one clock's gate (a
//! sequencer's gate, an LFO, a mix of signals). Its module counts the
//! input's rising edges, as it does inside a development; the drain counts
//! them too, from the same inputs, and writes each entry just before its
//! target computes the sample of its step's edge, splitting the target's
//! segment there as a condition's request does (see
//! `crate::invention::graph::conditions`).
//!
//! # Steps
//!
//! An edge is the clock input rising past [`GATE_LEVEL`]. Step 0 is the
//! first edge since the module started, or since its reset input last rose:
//! that restarts the table. Another plan plays on from the step counted:
//! entries at or before it have had their edge.
//!
//! # Where entries land
//!
//! On the edge's sample, for a target computing after every module feeding
//! the clock and reset inputs (one acyclic and not a sink): after the
//! requests, conditions and clock tables' entries due there. Any other
//! target (a sink, a feedback group's member, one computing before a source)
//! takes them at the end of the segment, late.
//!
//! An entry ends the drain's ramps in beats on its control. (Ramps measured
//! in edges follow; until then an entry's ramp jumps to its value.)

#![allow(dead_code)] // until the graph plays edges, next in this stack

use super::BeatHost;
use crate::control_request::hold::FrameScope;
use crate::control_request::ramps::Ramps;
use crate::control_request::table::{BeatTable, Counted};

/// The level an input rises past at an edge, as its module counts it.
pub(crate) const GATE_LEVEL: f32 = 0.5;

/// The targets a cursor remembers: a table writing more modules is looked
/// at for every module.
const TARGETS: usize = 8;

/// What edge cursors need of the graph, on top of [`BeatHost`].
pub(crate) trait EdgeHost: BeatHost {
    /// The current segment's first sample and length.
    fn segment(&self) -> (u64, usize);

    /// Input `port` of module `module` at frame `frame` of the segment:
    /// every route into it summed, as the module reads it (0 for none).
    fn input(&self, module: usize, port: Option<usize>, frame: usize) -> f32;

    /// Whether `target` is split on the segment's edges of `module`: an
    /// acyclic module, not a sink, computing after every module feeding
    /// `ports`.
    fn splits(&self, module: usize, ports: Ports, target: usize) -> bool;

    /// The installed index of the module named `id`.
    fn module_index(&self, id: &str) -> Option<usize>;
}

/// A table's module's clock and reset inputs, by index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Ports {
    pub(crate) clock: usize,
    pub(crate) reset: Option<usize>,
}

/// A count of edges, and the next entry from it.
#[derive(Clone, Copy)]
struct Count {
    counted: Counted,
    next: usize,
}

impl Count {
    /// Counts one frame of the inputs, as the module does. Returns whether
    /// the reset rose, and whether the gate did.
    fn frame(&mut self, gate: f32, reset: f32) -> (bool, bool) {
        let c = &mut self.counted;
        let restart = reset > GATE_LEVEL && c.reset <= GATE_LEVEL;
        let rise = gate > GATE_LEVEL && c.gate <= GATE_LEVEL;
        if restart {
            c.step = -1;
            c.measured = false;
            c.since = 0;
            self.next = 0;
        }
        if rise {
            if c.measured && c.since > 0 {
                c.interval = c.since;
            }
            c.measured = true;
            c.since = 0;
            c.step += 1;
        }
        c.since = c.since.saturating_add(1);
        (c.gate, c.reset) = (gate, reset);
        (restart, rise)
    }

    /// The step a frame with `rise` (and `restart`) would count.
    fn peek(&self, restart: bool) -> (i64, usize) {
        match restart {
            true => (0, 0),
            false => (self.counted.step + 1, self.next),
        }
    }
}

/// The modules a table's entries write, by index; `None` for more than
/// [`TARGETS`].
#[derive(Clone, Copy)]
struct Targets(Option<([usize; TARGETS], usize)>);

impl Targets {
    fn of(table: &dyn BeatTable, host: &impl EdgeHost) -> Self {
        let mut found = ([0; TARGETS], 0);
        for index in 0..table.len() {
            let Some(module) = target_of(table, index, host) else {
                continue;
            };
            if found.0[..found.1].contains(&module) {
                continue;
            }
            if found.1 == TARGETS {
                return Self(None);
            }
            found.0[found.1] = module;
            found.1 += 1;
        }
        Self(Some(found))
    }

    fn may_have(&self, module: usize) -> bool {
        self.0
            .is_none_or(|(found, len)| found[..len].contains(&module))
    }
}

struct EdgeCursor {
    /// The table's id ([`BeatTable::id`]), its module, and its inputs.
    table: usize,
    module: usize,
    ports: Ports,
    plan: u64,
    targets: Targets,
    /// Counted to the segment's start.
    count: Count,
    /// Counted on to frame `reached` for the target being played, if it
    /// splits on this table.
    scan: Count,
    reached: usize,
    live: bool,
    /// Found again since the graph last looked for tables.
    found: bool,
}

/// The tables a graph's drain plays on edges, one cursor each.
/// Single-threaded (the audio thread), allocated once on a control
/// thread and never grown.
pub(crate) struct EdgeCursors {
    entries: Vec<EdgeCursor>,
    limit: usize,
}

impl EdgeCursors {
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

    /// Whether it plays table `table`.
    pub(crate) fn has(&self, table: usize) -> bool {
        self.entries.iter().any(|cursor| cursor.table == table)
    }

    /// Starts looking for the graph's tables again: those not found by
    /// [`Self::sought`] end there.
    pub(crate) fn seek(&mut self) {
        for cursor in &mut self.entries {
            cursor.found = false;
        }
    }

    /// `table` is at `module`, counting edges on `ports`. Returns whether
    /// the drain plays it: with room. One new to it takes up its module's
    /// count. Allocation- and free-free.
    pub(crate) fn found(&mut self, table: &dyn BeatTable, module: usize, ports: Ports) -> bool {
        if let Some(cursor) = self.entries.iter_mut().find(|c| c.table == table.id()) {
            (cursor.module, cursor.ports, cursor.found) = (module, ports, true);
            return true;
        }
        if self.entries.len() >= self.limit {
            return false;
        }
        let counted = table.counted();
        let count = Count {
            counted,
            next: played(table, counted.step),
        };
        self.entries.push(EdgeCursor {
            table: table.id(),
            module,
            ports,
            plan: table.plan(),
            targets: Targets(None),
            count,
            scan: count,
            reached: 0,
            live: false,
            found: true,
        });
        true
    }

    /// Ends the cursors not found since [`Self::seek`], then puts the rest
    /// in module order and finds where their entries write. Allocation- and
    /// free-free.
    pub(crate) fn sought(&mut self, host: &impl EdgeHost) {
        let entries = &mut self.entries;
        entries.retain(|cursor| cursor.found);
        entries.sort_unstable_by_key(|cursor| cursor.module);
        for cursor in entries.iter_mut() {
            if let Some(table) = table_of(cursor, host) {
                cursor.targets = Targets::of(table, host);
            }
        }
    }

    /// Starts playing the segment for `target`, before it computes.
    pub(crate) fn begin(&mut self, target: usize, host: &impl EdgeHost) {
        for cursor in self.entries.iter_mut() {
            cursor.live = false;
            let Some(table) = table_of(cursor, host) else {
                continue;
            };
            sync(cursor, table, host);
            cursor.scan = cursor.count;
            cursor.reached = 0;
            cursor.live =
                cursor.targets.may_have(target) && host.splits(cursor.module, cursor.ports, target);
        }
    }

    /// Plays, for `target` (after [`Self::begin`]), what falls due at frame
    /// `frame` of the segment, before it computes that frame: every entry
    /// of a table counting an edge there. Returns the next frame something
    /// of it falls due at (the segment's length for none). Allocation-,
    /// free- and lock-free.
    pub(crate) fn play(
        &mut self,
        target: usize,
        frame: usize,
        host: &impl EdgeHost,
        beat_ramps: &mut Ramps,
    ) -> usize {
        let (_, frames) = host.segment();
        let mine = |module: Option<usize>| module == Some(target);
        let mut next = frames;
        for cursor in self.entries.iter_mut().filter(|cursor| cursor.live) {
            let Some(table) = table_of(cursor, host) else {
                continue;
            };
            // Frames counted ahead brought nothing of it.
            while cursor.reached <= frame {
                let at = cursor.reached;
                step(cursor, table, at, &mine, beat_ramps, host);
                cursor.reached += 1;
            }
            // Counts on, up to the next frame something of it is due at.
            while cursor.reached < frames {
                if due(cursor, table, cursor.reached, &mine, host) {
                    next = next.min(cursor.reached);
                    break;
                }
                let at = cursor.reached;
                step(cursor, table, at, &mine, beat_ramps, host);
                cursor.reached += 1;
            }
        }
        next
    }

    /// Counts the segment's edges for every table, at its end: plays, late,
    /// what falls due on targets not split on them, and keeps the count.
    /// Allocation-, free- and lock-free.
    pub(crate) fn commit(&mut self, host: &impl EdgeHost, beat_ramps: &mut Ramps) {
        let (_, frames) = host.segment();
        for cursor in self.entries.iter_mut() {
            let Some(table) = table_of(cursor, host) else {
                continue;
            };
            sync(cursor, table, host);
            let (module, ports) = (cursor.module, cursor.ports);
            let late = |target: Option<usize>| {
                !target.is_some_and(|target| host.splits(module, ports, target))
            };
            cursor.scan = cursor.count;
            for at in 0..frames {
                step(cursor, table, at, &late, beat_ramps, host);
            }
            cursor.count = cursor.scan;
        }
    }
}

/// The table `cursor` plays, while its module still has it.
fn table_of<'a>(cursor: &EdgeCursor, host: &'a impl EdgeHost) -> Option<&'a dyn BeatTable> {
    host.table(cursor.module)
        .filter(|table| table.id() == cursor.table)
}

/// The installed module entry `index` of `table` writes.
fn target_of(table: &dyn BeatTable, index: usize, host: &impl EdgeHost) -> Option<usize> {
    host.module_index(table.target_module(index)?)
}

/// The index of `table`'s first entry after step `step`.
fn played(table: &dyn BeatTable, step: i64) -> usize {
    let mut next = 0;
    while table
        .entry(next)
        .is_some_and(|entry| (entry.step as i64) <= step)
    {
        next += 1;
    }
    next
}

/// Follows another plan of `table`: entries at or before the step counted
/// have had their edge.
fn sync(cursor: &mut EdgeCursor, table: &dyn BeatTable, host: &impl EdgeHost) {
    if table.plan() == cursor.plan {
        return;
    }
    cursor.plan = table.plan();
    cursor.count.next = played(table, cursor.count.counted.step);
    cursor.targets = Targets::of(table, host);
}

/// Whether frame `at` brings something of `mine` due: an edge bringing
/// its entry.
fn due(
    cursor: &EdgeCursor,
    table: &dyn BeatTable,
    at: usize,
    mine: &impl Fn(Option<usize>) -> bool,
    host: &impl EdgeHost,
) -> bool {
    let gate = host.input(cursor.module, Some(cursor.ports.clock), at);
    let reset = host.input(cursor.module, cursor.ports.reset, at);
    let counted = &cursor.scan.counted;
    let rise = gate > GATE_LEVEL && counted.gate <= GATE_LEVEL;
    let restart = reset > GATE_LEVEL && counted.reset <= GATE_LEVEL;
    if !rise {
        return false;
    }
    let (step, mut next) = cursor.scan.peek(restart);
    while table
        .entry(next)
        .is_some_and(|entry| entry.step as i64 <= step)
    {
        if mine(target_of(table, next, host)) {
            return true;
        }
        next += 1;
    }
    false
}

/// Counts frame `at` of the segment for `cursor`, playing what falls due
/// there on targets `mine` takes. Returns whether an edge rose.
fn step(
    cursor: &mut EdgeCursor,
    table: &dyn BeatTable,
    at: usize,
    mine: &impl Fn(Option<usize>) -> bool,
    beat_ramps: &mut Ramps,
    host: &impl EdgeHost,
) -> bool {
    let gate = host.input(cursor.module, Some(cursor.ports.clock), at);
    let reset = host.input(cursor.module, cursor.ports.reset, at);
    let (_, rise) = cursor.scan.frame(gate, reset);
    if !rise {
        return false;
    }
    let (start, _) = host.segment();
    // Weighed against a higher tier's write as of its own sample.
    let _scope = FrameScope::enter(start + at as u64);
    let step = cursor.scan.counted.step;
    while let Some(entry) = table.entry(cursor.scan.next) {
        if entry.step as i64 > step {
            break;
        }
        let index = cursor.scan.next;
        cursor.scan.next += 1;
        if !mine(target_of(table, index, host)) {
            continue;
        }
        // A newer change supersedes the drain's ramps in beats there.
        beat_ramps.supersede_entry(&entry.target, host);
        entry.target.begin(0);
        entry.target.write(entry.value);
    }
    true
}
