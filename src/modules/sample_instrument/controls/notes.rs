//! An instrument's note events, on their way to the audio thread.
//!
//! Two bounded lock-free queues carry them: one for control threads'
//! notes, one for a scheduler's, which plays on the audio thread. A
//! control thread preempted mid-push holds up only its own queue (the
//! consumer sees nothing past a position claimed and not yet published),
//! so a scheduled note never waits behind one, and a control thread
//! sending notes as fast as it can never fills the scheduler's queue.
//!
//! Every event is stamped from one counter as it is sent. Each block the
//! module drains both queues and plays what it drained in stamp order: the
//! order the notes were sent in, even when two control threads' pushes
//! finish out of that order. A control thread's note still being pushed
//! plays a block later.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::control_request::{bounded, QueueConsumer, QueueProducer};

/// How many note events each queue holds between two blocks.
pub(crate) const NOTE_CAPACITY: usize = 256;

/// A note request, popped per block by the audio thread.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoteEvent {
    pub note: u8,
    pub on: bool,
    /// When it was sent, among every note sent to this instrument.
    stamp: u64,
}

/// The sending ends, held by the controls.
pub(in crate::modules::sample_instrument) struct NoteSenders {
    sent: QueueProducer<NoteEvent>,
    scheduled: QueueProducer<NoteEvent>,
    stamps: AtomicU64,
}

/// The receiving ends, held by the module. Audio thread.
pub(crate) struct NoteQueues {
    sent: QueueConsumer<NoteEvent>,
    scheduled: QueueConsumer<NoteEvent>,
    /// One block's events, sized for both queues full.
    drained: Vec<NoteEvent>,
}

/// An instrument's note queues. Allocates: control thread.
pub(in crate::modules::sample_instrument) fn note_queues() -> (NoteSenders, NoteQueues) {
    let (sent, sent_queue) = bounded(NOTE_CAPACITY);
    let (scheduled, scheduled_queue) = bounded(NOTE_CAPACITY);
    let senders = NoteSenders {
        sent,
        scheduled,
        stamps: AtomicU64::new(0),
    };
    let queues = NoteQueues {
        sent: sent_queue,
        scheduled: scheduled_queue,
        drained: Vec::with_capacity(2 * NOTE_CAPACITY),
    };
    (senders, queues)
}

impl NoteSenders {
    /// Queues a control thread's note, returning false when its queue is
    /// full. Lock-free and allocation-free.
    pub(in crate::modules::sample_instrument) fn send(&self, note: u8, on: bool) -> bool {
        self.sent.try_push(self.event(note, on)).is_ok()
    }

    /// Queues a scheduler's note, returning false when its queue is full.
    /// Lock-free and allocation-free.
    pub(in crate::modules::sample_instrument) fn schedule(&self, note: u8, on: bool) -> bool {
        self.scheduled.try_push(self.event(note, on)).is_ok()
    }

    fn event(&self, note: u8, on: bool) -> NoteEvent {
        let stamp = self.stamps.fetch_add(1, Ordering::Relaxed);
        NoteEvent { note, on, stamp }
    }
}

impl NoteQueues {
    /// Everything queued, earliest sent first: at most a queue's worth from
    /// each, so producers sending as fast as this drains cannot hold the
    /// block here. Wait-free and allocation-free.
    pub(crate) fn drain(&mut self) -> &[NoteEvent] {
        self.drained.clear();
        for queue in [&mut self.sent, &mut self.scheduled] {
            for _ in 0..NOTE_CAPACITY {
                let Some(event) = queue.pop() else { break };
                self.drained.push(event);
            }
        }
        // In place, so allocation-free; stamps are unique, so the order is
        // too.
        self.drained.sort_unstable_by_key(|event| event.stamp);
        &self.drained
    }

    /// The `index`th event the last [`drain`](Self::drain) returned.
    pub(crate) fn drained(&self, index: usize) -> NoteEvent {
        self.drained[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes(queues: &mut NoteQueues) -> Vec<(u8, bool)> {
        let drained = queues.drain().iter();
        drained.map(|event| (event.note, event.on)).collect()
    }

    #[test]
    fn notes_pop_in_the_order_they_were_sent() {
        let (senders, mut queues) = note_queues();
        assert!(senders.send(60, true));
        assert!(senders.schedule(64, true));
        assert!(senders.send(60, false));
        assert!(senders.schedule(67, true));
        let expected = [(60, true), (64, true), (60, false), (67, true)];
        assert_eq!(notes(&mut queues), expected);
    }

    #[test]
    fn notes_play_in_the_order_they_were_sent_when_pushes_finish_out_of_it() {
        let (senders, mut queues) = note_queues();
        // A control thread stamps a note on, and is preempted before it
        // claims a position; a scheduler's note off and another control
        // thread's note are sent and queued meanwhile.
        let preempted = senders.event(60, true);
        assert!(senders.schedule(60, false));
        assert!(senders.send(62, true));
        senders.sent.try_push(preempted).unwrap();
        let expected = [(60, true), (60, false), (62, true)];
        assert_eq!(notes(&mut queues), expected, "the note on is released");
    }

    #[test]
    fn draining_allocates_nothing() {
        let (senders, mut queues) = note_queues();
        for note in 0..NOTE_CAPACITY {
            assert!(senders.send(note as u8 % 128, true));
            assert!(senders.schedule(note as u8 % 128, false));
        }
        let (len, allocs, frees) = crate::alloc_counter::allocator_events(|| queues.drain().len());
        assert_eq!((len, allocs, frees), (2 * NOTE_CAPACITY, 0, 0));
    }

    #[test]
    fn a_scheduled_note_never_waits_behind_a_control_thread_mid_push() {
        let (senders, mut queues) = note_queues();
        // A control thread claimed a position, then was preempted before
        // publishing its note.
        let publish = senders.sent.claim().unwrap();
        assert!(senders.schedule(64, true));
        assert_eq!(notes(&mut queues), [(64, true)]);
        publish(senders.event(60, true));
        assert_eq!(notes(&mut queues), [(60, true)], "a block later");
    }

    #[test]
    fn control_threads_filling_their_queue_leave_the_schedulers_whole() {
        let (senders, mut queues) = note_queues();
        let sent = std::iter::repeat_with(|| senders.send(72, true));
        assert_eq!(sent.take_while(|&queued| queued).count(), NOTE_CAPACITY);
        for _ in 0..NOTE_CAPACITY {
            assert!(senders.schedule(64, true));
        }
        assert!(!senders.schedule(64, false), "its own queue full");
        assert_eq!(notes(&mut queues).len(), 2 * NOTE_CAPACITY);
    }
}
