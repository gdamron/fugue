//! A module's queued events: automation writes to its event controls kept
//! in the order made, every one of them, instead of coalescing in a slot.
//!
//! A slot keeping one write per block suits a trigger (two `reset`s in a
//! block reset once), not a note: a chord is several notes on one control
//! at one step. A module whose events must each play queues them here
//! (see [`ControlCells::queuing_events`](super::super::ControlCells)), and
//! its event controls' automation writes are applied one by one, in write
//! order, after its slots.
//!
//! Each write keeps the sample it was made for. Taken late (see
//! [`super`]), a value written for a later sample than an event queued is
//! queued too, before the events of its own sample and after those
//! earlier, so it applies after the events it followed and before those
//! it was written for ([`EventQueue::insert_value`]).
//!
//! Events are queued in sample order, those of one sample in the order
//! made, whatever order their writers ran in ([`EventQueue::push`]).
//!
//! The queue is bounded. A write finding it full is refused and counted,
//! as any refused automation write is. A module may keep its last places
//! for one control, its release, so a schedule crowding the queue with
//! starts can still stop them: a note off is not lost to note ons.
//!
//! Like the slots, the queue is written and taken on the audio thread
//! alone, in process order, so `Relaxed` throughout.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::super::cells::{decode, encode};
use super::super::request::{ControlIndex, RtValue};

/// The control index sits above the 34 bits a value is encoded in.
const INDEX_SHIFT: u32 = 48;

/// The bits a value is encoded in.
const VALUE_MASK: u64 = (1 << INDEX_SHIFT) - 1;

/// Marks a queued write's sample word as an event's.
const EVENT_BIT: u64 = 1 << 63;

/// A write made outside processing: for no known sample.
pub(crate) const NO_SAMPLE: u64 = EVENT_BIT - 1;

/// Places kept for a release control.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EventReserve {
    /// The control the places are kept for.
    pub(crate) control: ControlIndex,
    /// How many of the last places only it may take.
    pub(crate) places: usize,
}

/// A bounded queue of event writes, in sample order, then the order made.
pub(crate) struct EventQueue {
    words: Box<[AtomicU64]>,
    /// Per place, the sample its write was made for ([`NO_SAMPLE`] when
    /// unknown), with [`EVENT_BIT`] set for an event.
    samples: Box<[AtomicU64]>,
    len: AtomicUsize,
    reserve: Option<EventReserve>,
}

impl EventQueue {
    /// No queue: the module's events coalesce in their slots.
    pub(crate) fn none() -> Self {
        Self {
            words: Box::new([]),
            samples: Box::new([]),
            len: AtomicUsize::new(0),
            reserve: None,
        }
    }

    /// A queue of `capacity` writes, keeping the last places of `reserve`
    /// for its control. Allocates: control thread.
    pub(crate) fn new(capacity: usize, reserve: Option<EventReserve>) -> Self {
        Self {
            words: (0..capacity).map(|_| AtomicU64::new(0)).collect(),
            samples: (0..capacity).map(|_| AtomicU64::new(0)).collect(),
            len: AtomicUsize::new(0),
            reserve,
        }
    }

    /// Whether the module queues its events.
    #[inline]
    pub(crate) fn queues(&self) -> bool {
        !self.words.is_empty()
    }

    /// Queues an event write to `index` made for sample `at`
    /// ([`NO_SAMPLE`] when unknown), after every write made for `at` or
    /// earlier, so the queue stays in sample order when writers do not (two
    /// feedback groups each writing a whole segment), returning false when
    /// there is no room for it: the queue full, or, for any control but
    /// the reserve's, only reserved places left.
    #[inline]
    pub(crate) fn push(&self, index: ControlIndex, value: RtValue, at: u64) -> bool {
        let len = self.len.load(Ordering::Relaxed);
        if !self.has_room(index, len) {
            return false;
        }
        let at = at & NO_SAMPLE;
        let later = |position: usize| {
            let queued = self.samples[position].load(Ordering::Relaxed) & NO_SAMPLE;
            queued != NO_SAMPLE && queued > at
        };
        let place = (0..len).find(|&position| later(position)).unwrap_or(len);
        self.insert(place, len, index, value, at | EVENT_BIT);
        true
    }

    /// Queues a value write to `index` made for sample `at` when an event
    /// made for an earlier sample is queued: before the first event of
    /// `at` or later, after every write already queued for `at`. `None`
    /// when no such event is queued (the value goes in its slot, applying
    /// before every event), `Some(false)` when there is no room for it.
    #[inline]
    pub(crate) fn insert_value(
        &self,
        index: ControlIndex,
        value: RtValue,
        at: u64,
    ) -> Option<bool> {
        let len = self.len.load(Ordering::Relaxed);
        if at >= NO_SAMPLE {
            return None;
        }
        let sample = |position: usize| self.samples[position].load(Ordering::Relaxed);
        let follows = (0..len).any(|position| {
            let word = sample(position);
            word & EVENT_BIT != 0 && (word & NO_SAMPLE) < at
        });
        if !follows {
            return None;
        }
        if !self.has_room(index, len) {
            return Some(false);
        }
        let after = |word: u64| {
            let queued = word & NO_SAMPLE;
            queued != NO_SAMPLE && (queued > at || (queued == at && word & EVENT_BIT != 0))
        };
        let place = (0..len)
            .find(|&position| after(sample(position)))
            .unwrap_or(len);
        self.insert(place, len, index, value, at);
        Some(true)
    }

    /// Puts a write at `place` among `len` queued, shifting those from
    /// `place` on one later. The caller found room.
    fn insert(&self, place: usize, len: usize, index: ControlIndex, value: RtValue, sample: u64) {
        for position in (place..len).rev() {
            let word = self.words[position].load(Ordering::Relaxed);
            let at = self.samples[position].load(Ordering::Relaxed);
            self.words[position + 1].store(word, Ordering::Relaxed);
            self.samples[position + 1].store(at, Ordering::Relaxed);
        }
        self.place(place, index, value, sample);
        self.len.store(len + 1, Ordering::Relaxed);
    }

    /// Drops every value queued for `index`: a later one, finding no room
    /// behind the events, goes in its slot, and an older one queued must
    /// not overwrite it there.
    #[inline]
    pub(crate) fn remove_values(&self, index: ControlIndex) {
        let len = self.len.load(Ordering::Relaxed);
        let mut kept = 0;
        for position in 0..len {
            let word = self.words[position].load(Ordering::Relaxed);
            let sample = self.samples[position].load(Ordering::Relaxed);
            let value_of = sample & EVENT_BIT == 0 && (word >> INDEX_SHIFT) as u16 == index.0;
            if value_of {
                continue;
            }
            self.words[kept].store(word, Ordering::Relaxed);
            self.samples[kept].store(sample, Ordering::Relaxed);
            kept += 1;
        }
        self.len.store(kept, Ordering::Relaxed);
    }

    /// The sample the `position`th write was made for ([`NO_SAMPLE`] when
    /// unknown), and whether it is an event.
    #[inline]
    pub(crate) fn sample(&self, position: usize) -> Option<(u64, bool)> {
        let word = self.samples.get(position)?.load(Ordering::Relaxed);
        Some((word & NO_SAMPLE, word & EVENT_BIT != 0))
    }

    /// Whether a write to `index` finds room among `len` queued.
    fn has_room(&self, index: ControlIndex, len: usize) -> bool {
        let kept = match self.reserve {
            Some(reserve) if reserve.control != index => reserve.places,
            _ => 0,
        };
        len + kept < self.words.len()
    }

    fn place(&self, position: usize, index: ControlIndex, value: RtValue, sample: u64) {
        let word = (u64::from(index.0) << INDEX_SHIFT) | encode(value);
        self.words[position].store(word, Ordering::Relaxed);
        self.samples[position].store(sample, Ordering::Relaxed);
    }

    /// How many writes are queued.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed)
    }

    /// The `position`th write queued.
    #[inline]
    pub(crate) fn get(&self, position: usize) -> Option<(ControlIndex, RtValue)> {
        let word = self.words.get(position)?.load(Ordering::Relaxed);
        let index = ControlIndex((word >> INDEX_SHIFT) as u16);
        Some((index, decode(word & VALUE_MASK)))
    }

    /// Empties the queue.
    #[inline]
    pub(crate) fn clear(&self) {
        self.len.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: ControlIndex = ControlIndex(3);
    const OFF: ControlIndex = ControlIndex(4);

    fn queued(queue: &EventQueue) -> Vec<(u16, RtValue)> {
        (0..queue.len())
            .map(|position| {
                let (index, value) = queue.get(position).unwrap();
                (index.0, value)
            })
            .collect()
    }

    #[test]
    fn writes_keep_the_order_made_and_their_values() {
        let queue = EventQueue::new(8, None);
        assert!(queue.push(ON, RtValue::F32(60.0), NO_SAMPLE));
        assert!(queue.push(OFF, RtValue::F32(60.0), NO_SAMPLE));
        assert!(queue.push(ControlIndex(u16::MAX), RtValue::I32(-3), NO_SAMPLE));
        assert!(queue.push(ON, RtValue::Bool(true), NO_SAMPLE));
        let expected = [
            (3, RtValue::F32(60.0)),
            (4, RtValue::F32(60.0)),
            (u16::MAX, RtValue::I32(-3)),
            (3, RtValue::Bool(true)),
        ];
        assert_eq!(queued(&queue), expected);
        queue.clear();
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn the_last_places_take_only_the_reserved_control() {
        let reserve = EventReserve {
            control: OFF,
            places: 2,
        };
        let queue = EventQueue::new(4, Some(reserve));
        assert!(queue.push(ON, RtValue::F32(1.0), NO_SAMPLE));
        assert!(queue.push(ON, RtValue::F32(2.0), NO_SAMPLE));
        assert!(
            !queue.push(ON, RtValue::F32(3.0), NO_SAMPLE),
            "only reserved places left"
        );
        assert!(queue.push(OFF, RtValue::F32(1.0), NO_SAMPLE));
        assert!(queue.push(OFF, RtValue::F32(2.0), NO_SAMPLE));
        assert!(!queue.push(OFF, RtValue::F32(3.0), NO_SAMPLE), "full");
        assert_eq!(queue.len(), 4);
    }

    #[test]
    fn no_queue_queues_nothing() {
        let queue = EventQueue::none();
        assert!(!queue.queues());
        assert!(!queue.push(ON, RtValue::F32(1.0), NO_SAMPLE));
    }
}
