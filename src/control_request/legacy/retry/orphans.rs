//! Events a schedule leaves waiting when it stops trying them, kept at
//! their surface's gate for the schedules still writing it to land in turn.
//!
//! A scheduler an edit removes (see `Module::leave`) stops trying its
//! waiting writes. Its values are dropped, but its events (a `note_off`,
//! perhaps behind another scheduler's `note_on`) were made before the
//! edit, and would have landed on main. So each keeps its ticket at the
//! gate, in room of the gate's own, and the next schedule writing that
//! surface to retry lands it once every earlier ticket has, ahead of every
//! later one. When no schedule writes the surface any more, the last one
//! freed (off the audio thread) leaves the gate held, and the reclaimer
//! lands what it keeps on its cadence (see `land_left_events`): late, but
//! in order, and not lost to a surface busy or full for a while.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use super::gate::IDLE;
use super::{bits_value, value_bits, LegacyRefusal, LegacyWrites, Surface};

/// The events one gate keeps for schedules gone.
const ROOM: usize = 8;

/// Keys a gate names before it chains more.
const KEYS: usize = 16;

/// Events kept at every gate: none, and the audio thread looks no further.
static KEPT: AtomicUsize = AtomicUsize::new(0);

/// One event kept: its ticket ([`IDLE`] for none), control and value, and
/// whether a thread is landing it (the audio thread, or the reclaimer).
struct Orphan {
    ticket: AtomicU64,
    key: AtomicU32,
    value: AtomicU64,
    landing: AtomicBool,
}

impl Default for Orphan {
    fn default() -> Self {
        Self {
            ticket: AtomicU64::new(IDLE),
            key: AtomicU32::new(0),
            value: AtomicU64::new(0),
            landing: AtomicBool::new(false),
        }
    }
}

/// The names of the controls schedules write on one surface, by number:
/// named on a control thread, never forgotten, so the audio thread reads
/// them with no lock.
#[derive(Default)]
struct Keys {
    keys: [OnceLock<String>; KEYS],
    more: OnceLock<Box<Keys>>,
}

/// A gate's room for the events of schedules gone (see the module docs).
#[derive(Default)]
pub(super) struct Orphans {
    room: [Orphan; ROOM],
    kept: AtomicUsize,
    keys: Keys,
    /// The surface the events land on.
    surface: OnceLock<Surface>,
    /// Control threads only: one names a key at a time.
    naming: Mutex<()>,
}

impl Orphans {
    /// The number naming `surface`'s `key`. Control thread: may lock and
    /// allocate.
    pub(super) fn name(&self, surface: &Surface, key: &str) -> u32 {
        let _naming = self.naming.lock().unwrap_or_else(PoisonError::into_inner);
        self.surface.get_or_init(|| surface.clone());
        let mut keys = &self.keys;
        let mut base = 0;
        loop {
            for (at, slot) in keys.keys.iter().enumerate() {
                match slot.get() {
                    Some(named) if named == key => return (base + at) as u32,
                    Some(_) => {}
                    None => {
                        let _ = slot.set(key.to_string());
                        return (base + at) as u32;
                    }
                }
            }
            keys = keys.more.get_or_init(Box::default);
            base += KEYS;
        }
    }

    fn key(&self, name: u32) -> &str {
        let mut keys = &self.keys;
        let mut at = name as usize;
        while at >= KEYS {
            keys = keys.more.get().expect("a key this gate named");
            at -= KEYS;
        }
        keys.keys[at].get().expect("a key this gate named")
    }

    /// The earliest ticket kept, or [`IDLE`].
    pub(super) fn earliest(&self) -> u64 {
        // Acquire: none kept means every one cleared has landed.
        match self.kept.load(Ordering::Acquire) {
            0 => IDLE,
            _ => self.first().map_or(IDLE, |(_, ticket)| ticket),
        }
    }

    fn first(&self) -> Option<(usize, u64)> {
        (self.room.iter().enumerate())
            .map(|(at, orphan)| (at, orphan.ticket.load(Ordering::Acquire)))
            .filter(|&(_, ticket)| ticket != IDLE)
            .min_by_key(|&(_, ticket)| ticket)
    }

    /// Keeps an event with its ticket; false when there is no room.
    fn keep(&self, ticket: u64, key: u32, value: u64) -> bool {
        let free = self
            .room
            .iter()
            .find(|orphan| orphan.ticket.load(Ordering::Relaxed) == IDLE);
        let Some(orphan) = free else {
            return false;
        };
        self.kept.fetch_add(1, Ordering::Relaxed);
        KEPT.fetch_add(1, Ordering::Relaxed);
        orphan.key.store(key, Ordering::Relaxed);
        orphan.value.store(value, Ordering::Relaxed);
        orphan.ticket.store(ticket, Ordering::Release);
        true
    }

    /// How many events are kept.
    pub(super) fn kept(&self) -> usize {
        self.kept.load(Ordering::Relaxed)
    }

    /// Whether nothing but this room holds the surface: its module is gone
    /// from every graph, and nothing will make room on it again.
    pub(super) fn stranded(&self) -> bool {
        self.surface
            .get()
            .is_some_and(|surface| Arc::strong_count(surface) == 1)
    }

    /// Lands what is kept, in ticket order, while each one's turn has come:
    /// `due` gives the earliest ticket a live schedule has waiting, or
    /// `None` when that cannot be told now. Stops at one the surface
    /// refuses for now, or another thread is landing; returns how many the
    /// surface refused outright, dropped. Never waits.
    pub(super) fn land(&self, due: impl Fn() -> Option<u64>) -> u32 {
        let mut refused = 0;
        let Some(surface) = self.surface.get() else {
            return refused;
        };
        // `due` first: an event another schedule leaves after it is read,
        // earlier than those seen, then shows in `first`. Both are read
        // again once one is claimed (lanes before the room: an event is
        // left before its lane stops showing it).
        while let Some(earliest) = due() {
            let Some((at, ticket)) = self.first() else {
                break;
            };
            if ticket > earliest {
                break;
            }
            let orphan = &self.room[at];
            if orphan.landing.swap(true, Ordering::Acquire) {
                break;
            }
            // Claimed, it is looked at again: another thread may have
            // landed it, or (off the audio thread) an earlier write been
            // kept or left since `due` was read.
            let turn = due().is_some_and(|earliest| ticket <= earliest);
            if !turn || self.first().is_some_and(|(_, first)| first < ticket) {
                orphan.landing.store(false, Ordering::Release);
                break;
            }
            if orphan.ticket.load(Ordering::Acquire) == ticket {
                let key = self.key(orphan.key.load(Ordering::Relaxed));
                let value = bits_value(orphan.value.load(Ordering::Relaxed));
                match surface.set_scheduled(key, value) {
                    Err(LegacyRefusal::Busy | LegacyRefusal::Full) => {
                        orphan.landing.store(false, Ordering::Release);
                        break;
                    }
                    Err(_) => refused += 1,
                    Ok(()) => {}
                }
                self.clear(at);
            }
            orphan.landing.store(false, Ordering::Release);
        }
        refused
    }

    fn clear(&self, at: usize) {
        self.room[at].ticket.store(IDLE, Ordering::Release);
        self.kept.fetch_sub(1, Ordering::Release);
        KEPT.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Its events go with it: the audio thread need no longer look for them.
impl Drop for Orphans {
    fn drop(&mut self) {
        KEPT.fetch_sub(*self.kept.get_mut(), Ordering::Relaxed);
    }
}

impl LegacyWrites {
    /// Stops trying its writes: one last try each, in order; then each
    /// event still waiting is left at its surface's gate (see the module
    /// docs), and the rest are dropped, counted.
    pub(crate) fn abandon(&self) {
        self.retry();
        for slot in &self.slots[..self.waiting()] {
            let write = slot.load();
            let control = &self.controls[write.index];
            let orphans = &self.groups[control.group].gate.orphans;
            let bits = value_bits(write.value);
            if !(control.event && orphans.keep(write.ticket, control.name, bits)) {
                self.refused.record();
            }
        }
        self.waiting.store(0, Ordering::Relaxed);
        self.hide();
        self.settle();
    }

    /// Lands the events left at its surfaces' gates whose turn has come:
    /// every earlier ticket, this schedule's included, has landed. One a
    /// surface refuses outright is dropped, counted here.
    pub(super) fn land_orphans(&self) {
        if KEPT.load(Ordering::Relaxed) == 0 {
            return;
        }
        for group in self.groups.iter() {
            let gate = &group.gate;
            for _ in 0..gate.orphans.land(|| Some(gate.lanes_earliest())) {
                self.refused.record();
            }
        }
    }
}
