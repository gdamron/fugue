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

/// Places kept for a release control.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EventReserve {
    /// The control the places are kept for.
    pub(crate) control: ControlIndex,
    /// How many of the last places only it may take.
    pub(crate) places: usize,
}

/// A bounded queue of event writes, in the order made.
pub(crate) struct EventQueue {
    words: Box<[AtomicU64]>,
    len: AtomicUsize,
    reserve: Option<EventReserve>,
}

impl EventQueue {
    /// No queue: the module's events coalesce in their slots.
    pub(crate) fn none() -> Self {
        Self {
            words: Box::new([]),
            len: AtomicUsize::new(0),
            reserve: None,
        }
    }

    /// A queue of `capacity` writes, keeping the last places of `reserve`
    /// for its control. Allocates: control thread.
    pub(crate) fn new(capacity: usize, reserve: Option<EventReserve>) -> Self {
        Self {
            words: (0..capacity).map(|_| AtomicU64::new(0)).collect(),
            len: AtomicUsize::new(0),
            reserve,
        }
    }

    /// Whether the module queues its events.
    #[inline]
    pub(crate) fn queues(&self) -> bool {
        !self.words.is_empty()
    }

    /// Queues a write to `index`, returning false when there is no room
    /// for it: the queue full, or, for any control but the reserve's,
    /// only reserved places left.
    #[inline]
    pub(crate) fn push(&self, index: ControlIndex, value: RtValue) -> bool {
        let len = self.len.load(Ordering::Relaxed);
        let kept = match self.reserve {
            Some(reserve) if reserve.control != index => reserve.places,
            _ => 0,
        };
        if len + kept >= self.words.len() {
            return false;
        }
        let word = (u64::from(index.0) << INDEX_SHIFT) | encode(value);
        self.words[len].store(word, Ordering::Relaxed);
        self.len.store(len + 1, Ordering::Relaxed);
        true
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
        assert!(queue.push(ON, RtValue::F32(60.0)));
        assert!(queue.push(OFF, RtValue::F32(60.0)));
        assert!(queue.push(ControlIndex(u16::MAX), RtValue::I32(-3)));
        assert!(queue.push(ON, RtValue::Bool(true)));
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
        assert!(queue.push(ON, RtValue::F32(1.0)));
        assert!(queue.push(ON, RtValue::F32(2.0)));
        assert!(
            !queue.push(ON, RtValue::F32(3.0)),
            "only reserved places left"
        );
        assert!(queue.push(OFF, RtValue::F32(1.0)));
        assert!(queue.push(OFF, RtValue::F32(2.0)));
        assert!(!queue.push(OFF, RtValue::F32(3.0)), "full");
        assert_eq!(queue.len(), 4);
    }

    #[test]
    fn no_queue_queues_nothing() {
        let queue = EventQueue::none();
        assert!(!queue.queues());
        assert!(!queue.push(ON, RtValue::F32(1.0)));
    }
}
