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
//! back when it is dropped: its writes then hold no other back. A scheduler
//! an edit removes is freed only later, off the audio thread, so it gives
//! up what it has waiting as it leaves the graph (see `Module::leave`),
//! leaving its events here with their tickets for the schedules still
//! writing the surface to land in turn (see [`super::orphans`]). Tickets
//! need not land in order to be given back, so a write dropped from the
//! middle (one that waited too long) never holds the others up.

use std::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use super::orphans::Orphans;
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
    /// The ticket of the last event or resize kept, zero for none.
    last_barrier: AtomicU64,
    lanes: Lanes,
    /// Audio-thread passes begun and ended that hide lanes' heads before
    /// showing them again (see [`Self::settled_earliest`]).
    begun: AtomicU64,
    ended: AtomicU64,
    /// Events left waiting by schedules gone.
    pub(super) orphans: Orphans,
}

/// Every surface's gate, by the surface's address, and the gates held for
/// events left when no schedule writes their surface any more (see
/// [`super::orphans`]). Control threads only.
///
/// A gate lives while a schedule writing its surface does, and so does
/// the surface, so a live entry's address is still its surface's. A held
/// gate keeps its surface too.
struct Gates {
    all: Vec<(usize, Weak<Gate>)>,
    held: Vec<Arc<Gate>>,
}

static GATES: Mutex<Gates> = Mutex::new(Gates {
    all: Vec::new(),
    held: Vec::new(),
});

fn gates() -> MutexGuard<'static, Gates> {
    GATES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lands, in ticket order, the events kept at gates the last schedule
/// freed let go of, once no live schedule's earlier write waits there,
/// and lets go of the gates that then keep none, or whose surface nothing
/// else holds (its module gone, nothing will make room on it). One whose
/// surface is still busy or full keeps the rest for the next call. A
/// schedule built but not yet playing holds nothing back: it has nothing
/// waiting, and what it writes once playing waits behind them. Control
/// thread, on the reclaimer's cadence: never waits on a surface or the
/// audio thread.
pub(crate) fn land_left_events() {
    let mut gates = gates();
    let held = std::mem::take(&mut gates.held);
    let (still, done): (Vec<_>, Vec<_>) = held.into_iter().partition(|gate| {
        gate.orphans.land(|| gate.settled_earliest());
        let stranded = Arc::strong_count(gate) == 1 && gate.orphans.stranded();
        gate.orphans.kept() > 0 && !stranded
    });
    gates.held = still;
    drop(gates);
    // Freed unlocked: a surface freed with its gate may free schedules.
    drop(done);
}

impl Gate {
    /// The gate of `surface`, shared by every schedule writing it. Control
    /// thread: allocates and locks.
    pub(super) fn of(surface: &Surface) -> Arc<Gate> {
        let at = Arc::as_ptr(surface) as *const () as usize;
        let mut gates = gates();
        gates.all.retain(|(_, gate)| gate.strong_count() > 0);
        let live = (gates.all.iter()).find_map(|(of, gate)| (*of == at).then(|| gate.upgrade())?);
        live.unwrap_or_else(|| {
            let gate = Arc::new(Gate::default());
            gates.all.push((at, Arc::downgrade(&gate)));
            gate
        })
    }

    /// Holds `gate` while it keeps events, as a schedule writing its surface
    /// is freed: that may be the last (see [`land_left_events`]). Control
    /// thread.
    pub(super) fn hold_kept(gate: &Arc<Gate>) {
        if gate.orphans.kept() == 0 {
            return;
        }
        let mut gates = gates();
        if !gates.held.iter().any(|held| Arc::ptr_eq(held, gate)) {
            gates.held.push(gate.clone());
        }
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

    /// The earliest ticket waiting on the surface, from any schedule or
    /// left by one gone, or [`IDLE`] when none waits.
    pub(super) fn earliest(&self) -> u64 {
        self.lanes_earliest().min(self.orphans.earliest())
    }

    /// The earliest ticket a live schedule has waiting, or [`IDLE`].
    pub(super) fn lanes_earliest(&self) -> u64 {
        (self.lanes.iter())
            .map(|lane| lane.head.load(Ordering::Acquire))
            .min()
            .unwrap_or(IDLE)
    }

    /// Begins a pass that hides lane heads and shows them again. Audio
    /// thread.
    pub(super) fn begin(&self) {
        self.begun.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
    }

    /// Ends a pass begun.
    pub(super) fn end(&self) {
        self.ended.fetch_add(1, Ordering::Release);
    }

    /// [`Self::lanes_earliest`], read off the audio thread: `None` while
    /// a pass may have a head hidden.
    pub(super) fn settled_earliest(&self) -> Option<u64> {
        let begun = self.begun.load(Ordering::Acquire);
        if self.ended.load(Ordering::Acquire) != begun {
            return None;
        }
        let earliest = self.lanes_earliest();
        fence(Ordering::Acquire);
        (self.begun.load(Ordering::Relaxed) == begun).then_some(earliest)
    }

    /// Shows `ticket` as lane `lane`'s earliest waiting, or [`IDLE`].
    pub(super) fn show(&self, lane: usize, ticket: u64) {
        self.lane(lane).head.store(ticket, Ordering::Release);
    }

    /// Lane `lane`'s earliest ticket waiting, or [`IDLE`].
    pub(super) fn shown(&self, lane: usize) -> u64 {
        self.lane(lane).head.load(Ordering::Relaxed)
    }

    /// A ticket for a write kept now, after every one before it; an
    /// event's or a resize's is remembered (see [`Self::barrier_after`]).
    pub(super) fn ticket(&self, barrier: bool) -> u64 {
        let ticket = self.last.fetch_add(1, Ordering::Relaxed) + 1;
        if barrier {
            self.last_barrier.store(ticket, Ordering::Relaxed);
        }
        ticket
    }

    /// Whether `ticket` is the last kept on the surface: no write came
    /// between it and now.
    pub(super) fn is_last(&self, ticket: u64) -> bool {
        self.last.load(Ordering::Relaxed) == ticket
    }

    /// Whether an event or a resize was kept on the surface after
    /// `ticket`.
    pub(super) fn barrier_after(&self, ticket: u64) -> bool {
        self.last_barrier.load(Ordering::Relaxed) > ticket
    }
}
