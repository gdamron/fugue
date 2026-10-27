//! Control writes submitted to a live graph and not yet settled, for
//! read-back: reads return what was applied, and what is still on its way
//! is listed here.
//!
//! The log is the outcome channel's one consumer: it settles its own
//! writes, keeps every outcome it receives for whoever reports them (see
//! [`PendingLog::take_outcomes`]), records each applied write in the
//! session's request log (see [`crate::invention::request_log`]), and
//! lists each authored write that applied for the retained document to
//! record, in the order the audio thread applied them (see
//! [`PendingLog::take_steps`]). It also lists each front door's write
//! that applied for its `ControlChanged` event (see
//! [`PendingLog::take_announcements`]).

use std::collections::VecDeque;
use std::path::PathBuf;

use super::publisher::{OUTCOME_QUEUE_CAPACITY, REQUEST_QUEUE_CAPACITY, WAITING_CAPACITY};
use crate::control_request::{
    EventCursor, Outcome, OutcomeReceiver, RequestId, RtValue, Source, When,
};
use crate::invention::request_log::{
    LoggedBeat, LoggedEdit, LoggedRequest, LoggedWhen, Recorder, RequestLog,
};
use crate::ControlValue;

/// The most writes the log remembers: every request the queue, the
/// pending store and the watches can hold at once.
const LOG_CAPACITY: usize = REQUEST_QUEUE_CAPACITY + WAITING_CAPACITY;

/// Timed edits whose outcomes the log awaits. One waits at a time, so a
/// few more cover those adopted before reporting; past that, the oldest is
/// forgotten and its outcome counted lost, conservatively.
const TIMED_CAPACITY: usize = 8;

/// A control write waiting to be applied.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingWrite {
    pub(crate) module_id: String,
    pub(crate) key: String,
    pub(crate) value: ControlValue,
    /// The value exactly as the module will receive it, for the log; none
    /// for a payload, which the log cannot carry (it counts as unlogged).
    pub(crate) raw: Option<RtValue>,
    /// Its musical time, when it is timed in beats.
    pub(crate) beat: Option<LoggedBeat>,
    /// Who submitted it. One from automation, which takes a control back
    /// from a hold, a replay cannot reproduce: it holds every logged write
    /// above automation (so it counts as unlogged).
    pub(crate) source: Source,
    /// When it was submitted to apply.
    pub(crate) when: When,
    /// What the retained document records once the write applies, when it
    /// is authored.
    pub(crate) authored: Option<Authoring>,
    /// Announced as a `ControlChanged` event once it applies: a front
    /// door's write (see `RuntimeSnapshot::submit_control`), not a copy
    /// kept over a timed edit, which the edit owns, nor telemetry.
    pub(crate) announced: bool,
    /// A reply awaits its outcome, which is held apart from the shared
    /// ring of outcomes until the reply releases it (see
    /// [`PendingLog::release`]).
    pub(crate) awaited: bool,
}

/// A front door's write that applied, for its `ControlChanged` event, or
/// a timed one that settled, for its `ControlRequestSettled` event too.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Announcement {
    pub(crate) request: RequestId,
    pub(crate) module_id: String,
    pub(crate) key: String,
    pub(crate) value: ControlValue,
    /// What became of it: `None` for one whose outcome was lost, listed as
    /// applied, as most are.
    pub(crate) outcome: Option<Outcome>,
    /// Timed, announced however it settled, with its beat when timed in
    /// beats.
    pub(crate) timed: Option<Option<LoggedBeat>>,
}

impl Announcement {
    /// Whether it applied, as far as is known.
    pub(crate) fn applied(&self) -> bool {
        self.outcome.is_none_or(applied)
    }
}

/// Whether `outcome` says its request applied.
fn applied(outcome: Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Applied { .. } | Outcome::AppliedLate { .. }
    )
}

/// An authored write as the retained document records it once it applies:
/// the value as written (coerced to its control's kind, an asset path as
/// written) and the invention file a relative asset path was resolved
/// against, which the document may no longer be by then.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Authoring {
    pub(crate) value: ControlValue,
    pub(crate) origin: Option<PathBuf>,
    /// The timed edit the write is safe from (see `declared::Queued`),
    /// whose document does not replace it.
    pub(crate) safe: Option<RequestId>,
    /// Controls of its module a commit wrote while it waited, one of the
    /// two replacing the other, with the values the commit recorded: they
    /// applied after it, so they are recorded again after it is.
    pub(crate) then: Vec<(String, ControlValue)>,
    /// A timed write: ordered against a commit by when it applies, not by
    /// a fence (see `publish::timed_write`).
    pub(crate) timed: bool,
    /// A timed edit's own write, which its document holds already: recorded
    /// again only once a write since has changed what the document holds
    /// (see `publish::timed_chain`).
    pub(crate) chained: bool,
}

/// What the retained document takes from the outcomes, in the order the
/// audio thread reported them (see `LiveGraph::catch_up`).
#[derive(Debug)]
pub(crate) enum DocumentStep {
    /// An authored write applied, to be recorded.
    Authored {
        module_id: String,
        key: String,
        authoring: Authoring,
    },
    /// A timed edit settled, to be adopted first if it landed: an
    /// authored write applied after it applied to the graph it built.
    Timed,
}

/// Submitted writes, oldest first, until their outcome arrives, and the
/// outcomes received and not yet taken. Control threads only, behind the
/// live graph's lock for it, which a submitter takes under the publisher
/// (publisher, then this).
pub(crate) struct PendingLog {
    writes: VecDeque<(RequestId, PendingWrite)>,
    outcomes: VecDeque<(RequestId, Outcome)>,
    receiver: OutcomeReceiver,
    dropped: EventCursor,
    /// Outcomes lost since they were last taken: dropped by the audio
    /// thread for want of room, or evicted here unreported.
    lost: u64,
    /// The session's applied writes.
    log: Recorder,
    /// Timed edits queued (see `publish::timed`), until their outcomes,
    /// with what the request log records of each (none for one it cannot
    /// replay). More than the one waiting: one adopted may not have
    /// reported yet.
    timed: VecDeque<(RequestId, Option<LoggedEdit>)>,
    /// Timed edits' outcomes, until the publisher's ledger takes them, and
    /// whether any outcome was dropped meanwhile (see `timed_status`).
    timed_outcomes: VecDeque<(RequestId, Outcome)>,
    timed_lost: bool,
    /// What the retained document has yet to take, oldest first.
    steps: VecDeque<DocumentStep>,
    /// Writes applied and not yet announced, oldest first; past
    /// [`OUTCOME_QUEUE_CAPACITY`], the oldest goes unannounced.
    announcements: VecDeque<Announcement>,
    /// Outcomes of writes a reply awaits, until it releases them: never
    /// evicted, so a large batch's earlier outcomes stay known. Room for a
    /// batch is made before it is submitted (see [`Self::hold_room`]).
    held: Vec<(RequestId, Outcome)>,
}

impl PendingLog {
    pub(crate) fn new(receiver: OutcomeReceiver) -> Self {
        Self {
            writes: VecDeque::new(),
            outcomes: VecDeque::new(),
            receiver,
            dropped: EventCursor::new(),
            lost: 0,
            log: Recorder::default(),
            timed: VecDeque::new(),
            timed_outcomes: VecDeque::new(),
            timed_lost: false,
            steps: VecDeque::new(),
            announcements: VecDeque::new(),
            held: Vec::new(),
        }
    }

    /// Expects timed edit `id`'s outcome, never a lost write, and records
    /// `edit` with it in the request log as it settles, landed or refused
    /// (or counts a commit, for an edit the log cannot replay). Call it
    /// under this log's lock, taken before the submission, so the outcome
    /// cannot be received first.
    pub(crate) fn timed(&mut self, id: RequestId, edit: Option<LoggedEdit>) {
        if self.timed.len() == TIMED_CAPACITY {
            self.timed.pop_front();
        }
        self.timed.push_back((id, edit));
    }

    /// Receives every outcome waiting, settling the writes they belong to.
    /// When the audio thread had to drop outcomes, which writes they
    /// settled cannot be known, so the log forgets every write it holds
    /// rather than list one as pending for ever.
    pub(crate) fn settle(&mut self) {
        while let Some((id, outcome, latched)) = self.receiver.try_recv() {
            if let Some(at) = self.timed.iter().position(|(timed, _)| *timed == id) {
                let edit = self.timed.remove(at).and_then(|(_, edit)| edit);
                let landed = matches!(
                    outcome,
                    Outcome::Applied { .. } | Outcome::AppliedLate { .. }
                );
                match edit {
                    Some(edit) => {
                        // Refused on its beat, it latched its clock there.
                        if let (Some(at), LoggedWhen::Beat { beat }) = (latched, &edit.when) {
                            self.log.latched(at, beat.clock());
                        }
                        self.log.timed_edit(edit.settled(outcome));
                    }
                    // One the log cannot replay counts as a commit, if it
                    // landed or latched a clock.
                    None if landed || latched.is_some() => self.log.edited(),
                    None => {}
                }
                if self.timed_outcomes.len() == TIMED_CAPACITY {
                    self.timed_outcomes.pop_front();
                    self.timed_lost = true;
                }
                self.timed_outcomes.push_back((id, outcome));
                self.steps.push_back(DocumentStep::Timed);
            } else {
                let write = self.writes.iter().position(|(pending, _)| *pending == id);
                let write = write
                    .and_then(|at| self.writes.remove(at))
                    .map(|(_, write)| write);
                let applied = applied(outcome);
                if write.as_ref().is_some_and(|write| write.awaited) {
                    self.held.push((id, outcome));
                }
                if let Some(write) = write.as_ref() {
                    if applied {
                        self.authored(write);
                    }
                    self.announce(id, write, Some(outcome));
                }
                self.record(write, outcome, latched);
            }
            if self.outcomes.len() == OUTCOME_QUEUE_CAPACITY {
                self.outcomes.pop_front();
                self.lost += 1;
            }
            self.outcomes.push_back((id, outcome));
        }
        let dropped = self.dropped.take(self.receiver.dropped());
        if dropped > 0 {
            self.lost += u64::from(dropped);
            self.log.lost(u64::from(dropped));
            // Which applied cannot be known; most do, and an author's
            // write is not to be lost from the document.
            for (id, write) in std::mem::take(&mut self.writes) {
                self.authored(&write);
                self.announce(id, &write, None);
            }
            self.timed.clear();
            self.timed_lost = true;
        }
    }

    /// Timed edits' outcomes received since the last call, oldest first,
    /// and whether any outcome was lost meanwhile, a timed edit's maybe.
    pub(crate) fn take_timed(&mut self) -> (Vec<(RequestId, Outcome)>, bool) {
        self.settle();
        let lost = std::mem::take(&mut self.timed_lost);
        (self.timed_outcomes.drain(..).collect(), lost)
    }

    /// Records `write` in the session log when `outcome` applied it, or
    /// the clock it `latched` on its beat when it did not, or counts it
    /// lost when the write is no longer remembered.
    fn record(&mut self, write: Option<PendingWrite>, outcome: Outcome, latched: Option<u64>) {
        let (sample, due) = match outcome {
            Outcome::Applied { at } => (at, None),
            Outcome::AppliedLate { at, due } => (at, Some(due)),
            Outcome::Superseded | Outcome::Overridden | Outcome::Refused(_) => {
                let Some(at) = latched else {
                    return;
                };
                match write.as_ref().and_then(|write| write.beat.as_ref()) {
                    // A repeat may have lost more beats than it reports:
                    // the log cannot say where it latched.
                    Some(LoggedBeat::Grid { repeat: true, .. }) => self.log.unlogged(),
                    Some(beat) => self.log.latched(at, beat.clock()),
                    None => self.log.lost(1),
                }
                return;
            }
        };
        let Some(write) = write else {
            self.log.lost(1);
            return;
        };
        let Some(raw) = write.raw.filter(|_| write.source != Source::Automation) else {
            self.log.unlogged();
            return;
        };
        self.log.applied(LoggedRequest {
            sample,
            module_id: write.module_id,
            key: write.key,
            value: raw.into(),
            due,
            beat: write.beat,
        });
    }

    /// Counts a change committed to the graph, which a replay of the log
    /// does not reproduce.
    pub(crate) fn edited(&mut self) {
        self.log.edited();
    }

    /// Counts a write that reached the graph without a request.
    pub(crate) fn unlogged(&mut self) {
        self.log.unlogged();
    }

    /// The session's request log as recorded so far, every outcome
    /// received first.
    pub(crate) fn request_log(&mut self) -> RequestLog {
        self.settle();
        let mut log = self.log.export();
        // A repeat still waiting may have latched its clock on beats it
        // lost: it reports that only as it ends.
        let repeats = (self.writes.iter())
            .filter(|(_, write)| matches!(write.beat, Some(LoggedBeat::Grid { repeat: true, .. })))
            .count();
        log.unlogged += repeats as u64;
        log
    }

    /// Records a write just submitted as request `id`. Call it under the
    /// log's lock, taken before the submission, so the outcome cannot be
    /// received before the write is recorded.
    pub(crate) fn submitted(&mut self, id: RequestId, write: PendingWrite) {
        if self.writes.len() == LOG_CAPACITY {
            // Forgotten unsettled: recorded as the dropped are.
            if let Some((id, oldest)) = self.writes.pop_front() {
                self.authored(&oldest);
                self.announce(id, &oldest, None);
            }
        }
        self.writes.push_back((id, write));
    }

    /// Lists `write`, applied, for the document to record, if authored.
    fn authored(&mut self, write: &PendingWrite) {
        if let Some(authoring) = &write.authored {
            self.steps.push_back(DocumentStep::Authored {
                module_id: write.module_id.clone(),
                key: write.key.clone(),
                authoring: authoring.clone(),
            });
        }
    }

    /// Lists `write`, settled as request `id` as `outcome` says, for its
    /// events, if announced and it applied or was timed.
    fn announce(&mut self, id: RequestId, write: &PendingWrite, outcome: Option<Outcome>) {
        let timed = write.when != When::Now;
        if !write.announced || !(outcome.is_none_or(applied) || timed) {
            return;
        }
        if self.announcements.len() == OUTCOME_QUEUE_CAPACITY {
            self.announcements.pop_front();
        }
        self.announcements.push_back(Announcement {
            request: id,
            module_id: write.module_id.clone(),
            key: write.key.clone(),
            value: write.value.clone(),
            outcome,
            timed: timed.then(|| write.beat.clone()),
        });
    }

    /// Each front door's write applied since the last call, oldest first,
    /// every outcome received first. One whose outcome was lost is listed
    /// as applied: most do.
    pub(crate) fn take_announcements(&mut self) -> VecDeque<Announcement> {
        self.settle();
        std::mem::take(&mut self.announcements)
    }

    /// Request `id`'s outcome, when it is among those received and not
    /// yet taken, every outcome received first. `None` while it waits, and
    /// once its outcome is lost or taken.
    pub(crate) fn outcome_of(&mut self, id: RequestId) -> Option<Outcome> {
        self.settle();
        let held = self.held.iter().find(|(received, _)| *received == id);
        (held.into_iter().chain(self.outcomes.iter().rev()))
            .find(|(received, _)| *received == id)
            .map(|(_, outcome)| *outcome)
    }

    /// Whether request `id` still waits to apply.
    pub(crate) fn waiting(&self, id: RequestId) -> bool {
        self.writes.iter().any(|(waiting, _)| *waiting == id)
    }

    /// Makes room to hold the outcomes of `writes` more awaited writes, so
    /// receiving them allocates nothing.
    pub(crate) fn hold_room(&mut self, writes: usize) {
        self.held.reserve(writes);
    }

    /// Lets go of the held outcomes of `ids`, whose reply is built, and
    /// holds none of those still waiting once they settle.
    pub(crate) fn release(&mut self, ids: &[RequestId]) {
        self.held.retain(|(id, _)| !ids.contains(id));
        for (_, write) in self.writes.iter_mut().filter(|(id, _)| ids.contains(id)) {
            write.awaited = false;
        }
    }

    /// How many outcomes are held for replies.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.held.len()
    }

    /// The control of each authored write not yet recorded, waiting to
    /// apply or listed, but those safe from timed edit `except` and timed
    /// writes (see [`Authoring::timed`]).
    pub(crate) fn authored_keys(&self, except: Option<RequestId>) -> Vec<(String, String)> {
        let watched = |authoring: &Authoring| {
            !authoring.timed && (except.is_none() || authoring.safe != except)
        };
        let waiting = (self.writes.iter())
            .filter(|(_, write)| write.authored.as_ref().is_some_and(watched))
            .map(|(_, write)| (write.module_id.clone(), write.key.clone()));
        let listed = self.steps.iter().filter_map(|step| match step {
            DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } if watched(authoring) => Some((module_id.clone(), key.clone())),
            _ => None,
        });
        waiting.chain(listed).collect()
    }

    /// Leaves each authored write waiting to `module_id.key` for which
    /// `replaced` holds unrecorded when it applies (see `authored::Fence`).
    pub(crate) fn forget_authored(
        &mut self,
        replaced: impl Fn(&str, &str, &Authoring) -> bool,
        except: Option<RequestId>,
    ) {
        let safe = |authoring: &Authoring| except.is_some() && authoring.safe == except;
        for (_, write) in &mut self.writes {
            let forgotten = (write.authored.as_ref())
                .is_some_and(|a| !safe(a) && replaced(&write.module_id, &write.key, a));
            if forgotten {
                write.authored = None;
            }
        }
        // Listed since the publisher last took them: applied before the
        // commit was recorded, so as replaced.
        self.steps.retain(|step| match step {
            DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } => safe(authoring) || !replaced(module_id, key, authoring),
            DocumentStep::Timed => true,
        });
    }

    /// Adds to each authored write waiting, and each listed and not yet
    /// recorded, what `follows` says a commit wrote after it (see
    /// [`Authoring::then`]), but those safe from timed edit `except`, made
    /// after its writes.
    pub(crate) fn follow_authored(
        &mut self,
        follows: impl Fn(&str, &str) -> Vec<(String, ControlValue)>,
        except: Option<RequestId>,
    ) {
        let followed = |authoring: &Authoring| {
            !authoring.timed && (except.is_none() || authoring.safe != except)
        };
        let waiting = (self.writes.iter_mut()).filter_map(|(_, write)| {
            let authoring = write.authored.as_mut().filter(|a| followed(a))?;
            Some((write.module_id.as_str(), write.key.as_str(), authoring))
        });
        let listed = self.steps.iter_mut().filter_map(|step| match step {
            DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } if followed(authoring) => Some((module_id.as_str(), key.as_str(), authoring)),
            _ => None,
        });
        for (module_id, key, authoring) in waiting.chain(listed) {
            authoring.then.extend(follows(module_id, key));
        }
    }

    /// Whether an authored write waits to apply, or applied and is not yet
    /// recorded. A timed one waiting is not counted: it may wait long, and
    /// is recorded as it applies, after what is read now.
    pub(crate) fn authoring(&self) -> bool {
        !self.steps.is_empty()
            || (self.writes.iter())
                .any(|(_, write)| write.authored.as_ref().is_some_and(|a| !a.timed))
    }

    /// The modules of the timed authored writes waiting, or applied and
    /// not yet recorded, listed once each.
    pub(crate) fn timed_authored_modules(&self) -> Vec<String> {
        let waiting = (self.writes.iter())
            .filter_map(|(_, write)| Some((&write.module_id, write.authored.as_ref()?)));
        let applied = self.steps.iter().filter_map(|step| match step {
            DocumentStep::Authored {
                module_id,
                authoring,
                ..
            } => Some((module_id, authoring)),
            DocumentStep::Timed => None,
        });
        let mut modules: Vec<String> = Vec::new();
        for (module_id, authoring) in waiting.chain(applied) {
            if authoring.timed && !modules.contains(module_id) {
                modules.push(module_id.clone());
            }
        }
        modules
    }

    /// What the retained document has yet to take, every outcome received
    /// first: each authored write that applied and each timed edit that
    /// settled, in the order the audio thread reported them.
    pub(crate) fn take_steps(&mut self) -> VecDeque<DocumentStep> {
        self.settle();
        std::mem::take(&mut self.steps)
    }

    /// The writes still waiting, oldest first, each with its request.
    pub(crate) fn pending(&mut self) -> Vec<(RequestId, PendingWrite)> {
        self.settle();
        self.writes.iter().cloned().collect()
    }

    /// Every outcome received and not yet taken, oldest first, and how
    /// many were lost since the last call (never silently): what the front
    /// doors report.
    pub(crate) fn take_outcomes(&mut self) -> (Vec<(RequestId, Outcome)>, u64) {
        self.settle();
        let lost = std::mem::take(&mut self.lost);
        (self.outcomes.drain(..).collect(), lost)
    }
}
