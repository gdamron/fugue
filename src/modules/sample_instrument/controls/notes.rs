//! An instrument's note events, on their way to the audio thread.
//!
//! Bounded lock-free queues carry them: a few lanes for control threads'
//! notes, and one for a scheduler's, which plays on the audio thread. A
//! control thread takes a free lane for the length of one push, so a lane
//! never holds a note behind another thread's push still under way: a
//! control thread preempted mid-push holds up only its own lane, and only
//! its own note, which is still being sent. A scheduled note never waits
//! behind one, and control threads sending notes as fast as they can never
//! fill the scheduler's lane.
//!
//! Every event is stamped from one counter as it is sent, in its lane. Each
//! block the module takes, from every lane, the notes stamped before the
//! block began, and plays them in stamp order: the order the notes were
//! sent in, even when two control threads' pushes finish out of that
//! order. A note stamped later waits for the next block: otherwise a
//! thread's note pushed to a lane already drained could be overtaken by its
//! next note, in a lane drained after. A note whose push is held up past a
//! block plays the block after, as though sent when its push finished,
//! which it may be: its send had not returned.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::audio_thread::debug_assert_control_thread;
use crate::control_request::{bounded, QueueConsumer, QueueProducer};

/// How many note events each lane holds between two blocks.
pub(crate) const NOTE_CAPACITY: usize = 256;

/// How many lanes control threads share: as many pushes as can be under
/// way at once without one waiting for a lane.
pub(crate) const LANES: usize = 4;

/// How many times a control thread's send looks over the lanes for a free
/// one before giving up. Each look finds every lane not full taken by
/// another push, which lasts nanoseconds unless its thread is preempted;
/// a send never waits on threads that do not resume.
const SEND_ATTEMPTS: usize = 1024;

/// Places at the end of the scheduler's lane that only a note off may take,
/// so a schedule crowding its lane with note ons can still release them.
pub(crate) const NOTE_OFF_RESERVE: u64 = 16;

/// A note request, popped per block by the audio thread.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoteEvent {
    pub note: u8,
    pub on: bool,
    /// When it was sent, among every note sent to this instrument.
    pub stamp: u64,
}

/// A control threads' lane: one push at a time, by whoever takes it.
struct Lane {
    queue: QueueProducer<NoteEvent>,
    taken: AtomicBool,
}

/// The sending ends, held by the controls.
pub(in crate::modules::sample_instrument) struct NoteSenders {
    lanes: [Lane; LANES],
    /// The scheduler's lane: the audio thread pushes, never preempted by
    /// the drain it runs alongside.
    scheduled: QueueProducer<NoteEvent>,
    /// The next stamp.
    stamps: Arc<AtomicU64>,
}

/// The receiving ends, held by the module. Audio thread.
pub(crate) struct NoteQueues {
    lanes: [QueueConsumer<NoteEvent>; LANES],
    scheduled: QueueConsumer<NoteEvent>,
    /// The senders' next stamp, read as a block begins.
    stamps: Arc<AtomicU64>,
    /// One block's events, sized for every lane full.
    drained: Vec<NoteEvent>,
}

/// An instrument's note queues. Allocates: control thread.
pub(in crate::modules::sample_instrument) fn note_queues() -> (NoteSenders, NoteQueues) {
    let mut consumers = Vec::with_capacity(LANES);
    let lanes = std::array::from_fn(|_| {
        let (queue, consumer) = bounded(NOTE_CAPACITY);
        consumers.push(consumer);
        Lane {
            queue,
            taken: AtomicBool::new(false),
        }
    });
    let Ok(consumers) = <[_; LANES]>::try_from(consumers) else {
        unreachable!("one consumer per lane");
    };
    let (scheduled, scheduled_queue) = bounded(NOTE_CAPACITY);
    let stamps = Arc::new(AtomicU64::new(0));
    let senders = NoteSenders {
        lanes,
        scheduled,
        stamps: stamps.clone(),
    };
    let queues = NoteQueues {
        lanes: consumers,
        scheduled: scheduled_queue,
        stamps,
        drained: Vec::with_capacity((LANES + 1) * NOTE_CAPACITY),
    };
    (senders, queues)
}

impl NoteSenders {
    /// Queues a control thread's note in a free lane, returning false when
    /// every lane is full, or when every one not full had a push under way
    /// through [`SEND_ATTEMPTS`] looks, yielding between them. Lock-free
    /// and allocation-free.
    pub(in crate::modules::sample_instrument) fn send(&self, note: u8, on: bool) -> bool {
        debug_assert_control_thread("sending a sample instrument note");
        for _ in 0..SEND_ATTEMPTS {
            let mut contended = false;
            for lane in &self.lanes {
                let free =
                    lane.taken
                        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed);
                if free.is_err() {
                    contended = true;
                    continue;
                }
                let pushed = lane.queue.try_push(self.event(note, on)).is_ok();
                lane.taken.store(false, Ordering::Release);
                if pushed {
                    return true;
                }
            }
            if !contended {
                return false;
            }
            std::thread::yield_now();
        }
        false
    }

    /// Queues a scheduler's note, returning false when its lane is full,
    /// or, for a note on, when it would take one of the last
    /// [`NOTE_OFF_RESERVE`] places. Audio thread (one scheduler pushes at a
    /// time, so the reserve is exact); lock-free and allocation-free.
    pub(in crate::modules::sample_instrument) fn schedule(&self, note: u8, on: bool) -> bool {
        if on && !self.scheduled.leaves(NOTE_OFF_RESERVE) {
            return false;
        }
        self.scheduled.try_push(self.event(note, on)).is_ok()
    }

    /// Takes the first lane and claims a place in it for a control
    /// thread's note, returning what pushes the note and frees the lane, as
    /// a control thread preempted mid-push would.
    #[cfg(test)]
    pub(in crate::modules::sample_instrument) fn held_up(&self) -> impl FnOnce(u8, bool) + '_ {
        let lane = &self.lanes[0];
        let taken = lane.taken.swap(true, Ordering::Acquire);
        assert!(!taken, "the first lane is free");
        let publish = lane.queue.claim().unwrap();
        move |note, on| {
            publish(self.event(note, on));
            lane.taken.store(false, Ordering::Release);
        }
    }

    fn event(&self, note: u8, on: bool) -> NoteEvent {
        let stamp = self.stamps.fetch_add(1, Ordering::SeqCst);
        NoteEvent { note, on, stamp }
    }
}

impl NoteQueues {
    /// Everything queued and stamped before now, earliest sent first: at
    /// most a lane's worth from each lane, so producers sending as fast as
    /// this drains cannot hold the block here. Wait-free and
    /// allocation-free.
    pub(crate) fn drain(&mut self) -> &[NoteEvent] {
        self.drained.clear();
        // Each lane's stamps rise, as one sender at a time stamps and
        // pushes; a note stamped before this was sent before this, and so
        // was every note its sender sent before it.
        let before = self.stamps.load(Ordering::SeqCst);
        for queue in self.lanes.iter_mut().chain([&mut self.scheduled]) {
            for _ in 0..NOTE_CAPACITY {
                if queue.peek(|event| event.stamp < before) != Some(true) {
                    break;
                }
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
        // A control thread stamps a note on in the first lane, and is
        // preempted before it pushes; a scheduler's note off and another
        // control thread's note are sent and queued meanwhile.
        senders.lanes[0].taken.store(true, Ordering::Relaxed);
        let preempted = senders.event(60, true);
        assert!(senders.schedule(60, false));
        assert!(senders.send(62, true));
        senders.lanes[0].queue.try_push(preempted).unwrap();
        senders.lanes[0].taken.store(false, Ordering::Relaxed);
        let expected = [(60, true), (60, false), (62, true)];
        assert_eq!(notes(&mut queues), expected, "the note on is released");
    }

    #[test]
    fn a_note_stamped_before_a_block_and_pushed_after_plays_the_block_after() {
        let (senders, mut queues) = note_queues();
        // A control thread stamps a note on in the first lane and is
        // preempted before it pushes, across a whole block; another
        // thread's note plays meanwhile.
        senders.lanes[0].taken.store(true, Ordering::Relaxed);
        let preempted = senders.event(60, true);
        assert!(senders.send(62, true));
        assert_eq!(notes(&mut queues), [(62, true)]);
        senders.lanes[0].queue.try_push(preempted).unwrap();
        senders.lanes[0].taken.store(false, Ordering::Relaxed);
        assert!(senders.send(64, true));
        let expected = [(60, true), (64, true)];
        assert_eq!(notes(&mut queues), expected, "as though sent then");
    }

    #[test]
    fn a_scheduled_note_off_finds_room_note_ons_cannot_take() {
        let (senders, mut queues) = note_queues();
        let mut ons = 0;
        while senders.schedule(60, true) {
            ons += 1;
        }
        let reserve = NOTE_OFF_RESERVE as usize;
        assert_eq!(ons, NOTE_CAPACITY - reserve);
        for _ in 0..reserve {
            assert!(senders.schedule(60, false), "a note off has room");
        }
        assert!(!senders.schedule(60, false), "the lane is full");
        assert_eq!(queues.drain().len(), NOTE_CAPACITY);
    }

    #[test]
    fn draining_allocates_nothing() {
        let (senders, mut queues) = note_queues();
        for note in 0..LANES * NOTE_CAPACITY {
            assert!(senders.send(note as u8 % 128, true));
        }
        for note in 0..NOTE_CAPACITY {
            assert!(senders.schedule(note as u8 % 128, false));
        }
        let (len, allocs, frees) = crate::alloc_counter::allocator_events(|| queues.drain().len());
        assert_eq!((len, allocs, frees), ((LANES + 1) * NOTE_CAPACITY, 0, 0));
    }

    #[test]
    fn a_note_sent_while_another_push_is_held_up_plays_without_waiting() {
        let (senders, mut queues) = note_queues();
        // A control thread took a lane and claimed a place in it, then was
        // preempted before publishing its note. Another control thread's
        // note and a scheduler's play regardless, in the order sent.
        let publish = senders.held_up();
        assert!(senders.send(60, true));
        assert!(senders.schedule(60, false));
        assert_eq!(notes(&mut queues), [(60, true), (60, false)]);
        publish(72, true);
        assert_eq!(notes(&mut queues), [(72, true)], "a block later");
    }

    /// Sends a note on another thread, failing rather than hanging should
    /// the send wait.
    fn send_soon(senders: &Arc<NoteSenders>, note: u8) -> bool {
        let (sent, received) = std::sync::mpsc::channel();
        let senders = senders.clone();
        std::thread::spawn(move || sent.send(senders.send(note, true)));
        let patience = std::time::Duration::from_secs(10);
        received
            .recv_timeout(patience)
            .expect("the send waited for a lane")
    }

    #[test]
    fn a_note_with_no_free_lane_is_refused_while_their_pushes_stay_held_up() {
        let (senders, mut queues) = note_queues();
        let senders = Arc::new(senders);
        // Every lane taken by a control thread preempted mid-push: a send
        // gives up rather than wait for them to resume.
        for lane in &senders.lanes {
            lane.taken.store(true, Ordering::Relaxed);
        }
        assert!(!send_soon(&senders, 60));
        // One lane free again, the others full: still refused.
        for lane in &senders.lanes[1..] {
            lane.taken.store(false, Ordering::Relaxed);
            while lane.queue.try_push(senders.event(72, true)).is_ok() {}
        }
        assert!(!send_soon(&senders, 60));
        // The first lane's push finishes: the note goes in there.
        senders.lanes[0].taken.store(false, Ordering::Relaxed);
        assert!(senders.send(60, true));
        assert_eq!(notes(&mut queues).last(), Some(&(60, true)));
    }

    #[test]
    fn control_threads_filling_their_lanes_leave_the_schedulers_whole() {
        let (senders, mut queues) = note_queues();
        let sent = std::iter::repeat_with(|| senders.send(72, true));
        let queued = sent.take_while(|&queued| queued).count();
        assert_eq!(queued, LANES * NOTE_CAPACITY);
        let reserve = NOTE_OFF_RESERVE as usize;
        for _ in 0..NOTE_CAPACITY - reserve {
            assert!(senders.schedule(64, true));
        }
        for _ in 0..reserve {
            assert!(senders.schedule(64, false));
        }
        assert!(!senders.schedule(64, false), "its own lane full");
        assert_eq!(notes(&mut queues).len(), (LANES + 1) * NOTE_CAPACITY);
    }

    /// Eight threads send notes as fast as they can while this drains:
    /// each thread's play in the order it sent them, and none is lost.
    fn eight_threads_send_while_draining() {
        const THREADS: u8 = 8;
        const EACH: u8 = 120;
        let (senders, mut queues) = note_queues();
        let senders = Arc::new(senders);
        let threads: Vec<_> = (0..THREADS)
            .map(|thread| {
                let senders = senders.clone();
                std::thread::spawn(move || {
                    for i in 0..EACH {
                        // The note carries the count; on/off, the thread's
                        // parity, so a thread's notes are told apart below.
                        while !senders.send(i, thread % 2 == 0) {
                            std::thread::yield_now();
                        }
                    }
                })
            })
            .collect();
        // Per thread parity, the counts played so far: each thread's notes
        // arrive in order, so per parity they never go back past any
        // thread's last.
        let mut played = vec![Vec::new(), Vec::new()];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while played.iter().map(Vec::len).sum::<usize>() < usize::from(THREADS) * usize::from(EACH)
        {
            assert!(std::time::Instant::now() < deadline, "notes went missing");
            for event in queues.drain() {
                played[usize::from(!event.on)].push(event.note);
            }
        }
        for thread in threads {
            thread.join().unwrap();
        }
        assert!(queues.drain().is_empty());
        for counts in played {
            // Four threads per parity, each in order: every count shows up
            // four times, and the k-th time a count shows up, the count
            // before it has shown up at least k times.
            let mut seen = [0usize; EACH as usize];
            for count in counts {
                let count = usize::from(count);
                seen[count] += 1;
                if count > 0 {
                    assert!(seen[count - 1] >= seen[count], "out of order at {count}");
                }
            }
            assert!(seen.iter().all(|&times| times == usize::from(THREADS / 2)));
        }
    }

    #[test]
    fn each_threads_notes_play_in_the_order_it_sent_them_and_none_is_lost() {
        for _ in 0..10 {
            eight_threads_send_while_draining();
        }
    }
}
