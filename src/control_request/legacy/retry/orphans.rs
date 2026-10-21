//! Events a schedule leaves waiting when it stops trying them, kept at
//! their surface's gate for the schedules still writing it to land in turn.
//!
//! A scheduler an edit removes (see `Module::leave`) stops trying its
//! waiting writes. Its values are dropped, but its events (a `note_off`,
//! perhaps behind another scheduler's `note_on`) were made before the
//! edit, and would have landed on main. So each keeps its ticket at the
//! gate, in room of the gate's own, and the next schedule writing that
//! surface to retry lands it once every earlier ticket has, ahead of every
//! later one. When no schedule writes the surface any more, the gate goes
//! with the last of them, freed off the audio thread, and lands what it
//! still keeps then, through the setter, as main's waiting write would
//! have: late, but never lost while the graph reclaims what it retires.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use super::gate::IDLE;
use super::{bits_value, value_bits, LegacyRefusal, LegacyWrites, Surface};
use crate::control_request::table::TableValue;
use crate::ControlValue;

/// The events one gate keeps for schedules gone.
const ROOM: usize = 8;

/// Keys a gate names before it chains more.
const KEYS: usize = 16;

/// Events kept at every gate: none, and the audio thread looks no further.
static KEPT: AtomicUsize = AtomicUsize::new(0);

/// One event kept: its ticket ([`IDLE`] for none), control and value.
struct Orphan {
    ticket: AtomicU64,
    key: AtomicU32,
    value: AtomicU64,
}

impl Default for Orphan {
    fn default() -> Self {
        Self {
            ticket: AtomicU64::new(IDLE),
            key: AtomicU32::new(0),
            value: AtomicU64::new(0),
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
    /// The surface, for what is still kept when the gate goes.
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
        match self.kept.load(Ordering::Relaxed) {
            0 => IDLE,
            _ => self.first().map_or(IDLE, |(_, ticket)| ticket),
        }
    }

    fn first(&self) -> Option<(usize, u64)> {
        (self.room.iter().enumerate())
            .map(|(at, orphan)| (at, orphan.ticket.load(Ordering::Relaxed)))
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
        orphan.key.store(key, Ordering::Relaxed);
        orphan.value.store(value, Ordering::Relaxed);
        orphan.ticket.store(ticket, Ordering::Relaxed);
        self.kept.fetch_add(1, Ordering::Relaxed);
        KEPT.fetch_add(1, Ordering::Relaxed);
        true
    }

    fn clear(&self, at: usize) {
        self.room[at].ticket.store(IDLE, Ordering::Relaxed);
        self.kept.fetch_sub(1, Ordering::Relaxed);
        KEPT.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The last schedule writing the surface is gone, freed off the audio
/// thread: what is still kept lands now, in order, through the setter, which
/// may wait for the surface's lock as main's write did.
impl Drop for Orphans {
    fn drop(&mut self) {
        while let Some((at, _)) = self.first() {
            if let Some(surface) = self.surface.get() {
                let key = self.key(self.room[at].key.load(Ordering::Relaxed));
                let value = match bits_value(self.room[at].value.load(Ordering::Relaxed)) {
                    TableValue::Number(number) => ControlValue::Number(number),
                    TableValue::Bool(flag) => ControlValue::Bool(flag),
                };
                let _ = surface.set_control(key, value);
            }
            self.clear(at);
        }
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
    }

    /// Lands the events left at its surfaces' gates whose turn has come:
    /// every earlier ticket, this schedule's included, has landed. One a
    /// surface refuses outright is dropped, counted here.
    pub(super) fn land_orphans(&self) {
        if KEPT.load(Ordering::Relaxed) == 0 {
            return;
        }
        for group in self.groups.iter() {
            let orphans = &group.gate.orphans;
            while let Some((at, ticket)) = orphans.first() {
                if ticket > group.gate.lanes_earliest() {
                    break;
                }
                let orphan = &orphans.room[at];
                let key = orphans.key(orphan.key.load(Ordering::Relaxed));
                let value = bits_value(orphan.value.load(Ordering::Relaxed));
                match group.surface.set_scheduled(key, value) {
                    Ok(()) => orphans.clear(at),
                    Err(LegacyRefusal::Busy | LegacyRefusal::Full) => break,
                    Err(_) => {
                        orphans.clear(at);
                        self.refused.record();
                    }
                }
            }
        }
    }
}
