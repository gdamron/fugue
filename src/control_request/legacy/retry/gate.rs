//! The order of the writes waiting on one legacy surface, across every
//! schedule writing it.
//!
//! Each schedule keeps its own waiting writes (see [`super::LegacyWrites`]),
//! so two schedulers writing one instrument would each keep their own in
//! order, but not each other's: a `note_off` one kept after the other's
//! `note_on` could land first, and the note stick. So every write kept
//! takes a ticket from its surface's [`Gate`], and each schedule shows the
//! gate the earliest ticket it has waiting there, in a lane of its own. A
//! write is tried only while no earlier ticket waits, from any schedule,
//! and a new one goes straight to the surface only while none waits at all.
//!
//! A lane is claimed on a control thread as a schedule is built, and given
//! back when it is dropped: its writes then hold no other back. Tickets
//! need not land in order to be given back, so a write dropped from the
//! middle (one that waited too long) never holds the others up.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use super::Surface;

/// A lane's earliest ticket when nothing of its schedule waits.
pub(super) const IDLE: u64 = u64::MAX;

/// Lanes a gate holds before it chains more.
const LANES: usize = 8;

/// One schedule's place at a gate.
struct Lane {
    owned: AtomicBool,
    /// The earliest ticket its schedule has waiting, or [`IDLE`].
    head: AtomicU64,
}

impl Default for Lane {
    fn default() -> Self {
        Self {
            owned: AtomicBool::new(false),
            head: AtomicU64::new(IDLE),
        }
    }
}

/// A gate's lanes: room for [`LANES`], and more chained on a control
/// thread when every one is claimed. Never shrinks, so the audio thread
/// reads it with no lock.
#[derive(Default)]
struct Lanes {
    lanes: [Lane; LANES],
    more: OnceLock<Box<Lanes>>,
}

impl Lanes {
    fn iter(&self) -> impl Iterator<Item = &Lane> {
        let mut at = Some(self);
        std::iter::from_fn(move || {
            let lanes = at?;
            at = lanes.more.get().map(Box::as_ref);
            Some(lanes)
        })
        .flat_map(|lanes| lanes.lanes.iter())
    }
}

/// The order of the writes waiting on one surface (see the module docs).
#[derive(Default)]
pub(super) struct Gate {
    /// The last ticket handed out: zero before the first.
    last: AtomicU64,
    /// The ticket of the last event kept, zero for none.
    last_event: AtomicU64,
    lanes: Lanes,
}

/// Every surface's gate, by the surface's address. Control threads only.
///
/// A gate lives while a schedule writing its surface does, and so does
/// the surface, so a live entry's address is still its surface's.
static GATES: Mutex<Vec<(usize, Weak<Gate>)>> = Mutex::new(Vec::new());

impl Gate {
    /// The gate of `surface`, shared by every schedule writing it. Control
    /// thread: allocates and locks.
    pub(super) fn of(surface: &Surface) -> Arc<Gate> {
        let at = Arc::as_ptr(surface) as *const () as usize;
        let mut gates = GATES.lock().unwrap_or_else(PoisonError::into_inner);
        gates.retain(|(_, gate)| gate.strong_count() > 0);
        let live = (gates.iter()).find_map(|(of, gate)| (*of == at).then(|| gate.upgrade())?);
        live.unwrap_or_else(|| {
            let gate = Arc::new(Gate::default());
            gates.push((at, Arc::downgrade(&gate)));
            gate
        })
    }

    /// Claims a lane, chaining more when every one is taken. Control
    /// thread: may allocate.
    pub(super) fn claim(&self) -> usize {
        let mut lanes = &self.lanes;
        let mut base = 0;
        loop {
            let free = (lanes.lanes.iter()).position(|lane| {
                (lane.owned)
                    .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            });
            if let Some(at) = free {
                return base + at;
            }
            lanes = lanes.more.get_or_init(Box::default);
            base += LANES;
        }
    }

    /// Gives lane `lane` back, holding nothing.
    pub(super) fn release(&self, lane: usize) {
        let lane = self.lane(lane);
        lane.head.store(IDLE, Ordering::Relaxed);
        lane.owned.store(false, Ordering::Release);
    }

    fn lane(&self, lane: usize) -> &Lane {
        let lanes = (self.lanes.iter()).nth(lane);
        lanes.expect("a lane this gate handed out")
    }

    /// The earliest ticket waiting on the surface, from any schedule, or
    /// [`IDLE`] when none waits.
    pub(super) fn earliest(&self) -> u64 {
        (self.lanes.iter())
            .map(|lane| lane.head.load(Ordering::Relaxed))
            .min()
            .unwrap_or(IDLE)
    }

    /// Shows `ticket` as lane `lane`'s earliest waiting, or [`IDLE`].
    pub(super) fn show(&self, lane: usize, ticket: u64) {
        self.lane(lane).head.store(ticket, Ordering::Relaxed);
    }

    /// Lane `lane`'s earliest ticket waiting, or [`IDLE`].
    pub(super) fn shown(&self, lane: usize) -> u64 {
        self.lane(lane).head.load(Ordering::Relaxed)
    }

    /// A ticket for a write kept now, after every one before it; an
    /// event's is remembered (see [`Self::event_after`]).
    pub(super) fn ticket(&self, event: bool) -> u64 {
        let ticket = self.last.fetch_add(1, Ordering::Relaxed) + 1;
        if event {
            self.last_event.store(ticket, Ordering::Relaxed);
        }
        ticket
    }

    /// Whether `ticket` is the last kept on the surface: no write came
    /// between it and now.
    pub(super) fn is_last(&self, ticket: u64) -> bool {
        self.last.load(Ordering::Relaxed) == ticket
    }

    /// Whether an event was kept on the surface after `ticket`.
    pub(super) fn event_after(&self, ticket: u64) -> bool {
        self.last_event.load(Ordering::Relaxed) > ticket
    }
}
