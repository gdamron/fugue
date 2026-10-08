//! Payloads: applying one is allocation- and free-free, retired values drop
//! on the draining thread, a saturated retire path holds and then defers
//! rather than dropping on the audio thread, the producer never waits for a
//! stalled consumer, and the debug check catches an audio-thread drop.

use std::sync::mpsc::{self, Receiver, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use super::{Payload, RetireQueue, Retired, Retirer, Shared, MAX_RETIRES_PER_REQUEST, RETIRE_HOLD};
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;

/// Records which thread drops each [`Tracked`] value.
#[derive(Clone, Default)]
struct Ledger(Arc<Mutex<Vec<(usize, ThreadId)>>>);

impl Ledger {
    fn value(&self, id: usize) -> Tracked {
        Tracked {
            id,
            ledger: self.clone(),
        }
    }

    fn drops(&self) -> Vec<(usize, ThreadId)> {
        self.0.lock().unwrap().clone()
    }

    fn dropped_ids(&self) -> Vec<usize> {
        let mut ids: Vec<usize> = self.drops().into_iter().map(|(id, _)| id).collect();
        ids.sort_unstable();
        ids
    }
}

struct Tracked {
    id: usize,
    ledger: Ledger,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        let dropper = thread::current().id();
        self.ledger.0.lock().unwrap().push((self.id, dropper));
    }
}

/// A stand-in module that keeps its current value as a `Shared<T>`.
struct Module<T> {
    current: Shared<T>,
}

impl<T: Send + Sync + 'static> Module<T> {
    /// Applies a payload the way a module's request handler would: adopt it
    /// and retire the replaced value, or retire it if it has the wrong type.
    fn apply(&mut self, payload: Payload, retirer: &mut Retirer) {
        match payload.downcast::<T>() {
            Ok(next) => retirer.retire(std::mem::replace(&mut self.current, next)),
            Err(refused) => retirer.retire(refused),
        }
    }
}

/// The audio side of one block, as the request drain will run it: flush,
/// then take requests while a whole request's retirements are sure to fit.
fn run_block<T: Send + Sync + 'static>(
    requests: &Receiver<Payload>,
    retirer: &mut Retirer,
    module: &mut Module<T>,
) -> usize {
    retirer.flush();
    let mut applied = 0;
    while retirer.has_room(MAX_RETIRES_PER_REQUEST) {
        let Ok(payload) = requests.try_recv() else {
            break;
        };
        module.apply(payload, retirer);
        applied += 1;
    }
    applied
}

/// Runs `f` on `state` on a fresh thread standing in for the audio thread,
/// inside an [`AudioThreadScope`], asserting it neither allocates nor frees.
/// Returns its result and the thread.
fn on_audio<S: Send, R: Send>(state: &mut S, f: impl FnOnce(&mut S) -> R + Send) -> (R, ThreadId) {
    thread::scope(|scope| {
        scope
            .spawn(|| {
                let audio = thread::current().id();
                let (result, allocs, frees) = allocator_events(|| {
                    let _scope = AudioThreadScope::enter();
                    f(state)
                });
                assert_eq!((allocs, frees), (0, 0), "audio side allocated or freed");
                (result, audio)
            })
            .join()
            .unwrap()
    })
}

/// Drains `queue` on a fresh control thread. Returns the count and thread.
fn drain_on_control(queue: &RetireQueue) -> (usize, ThreadId) {
    thread::scope(|scope| {
        scope
            .spawn(|| (queue.drain(), thread::current().id()))
            .join()
            .unwrap()
    })
}

#[test]
fn applying_a_payload_allocates_and_frees_nothing() {
    let queue = RetireQueue::with_capacity(4);
    let (submit, requests) = mpsc::sync_channel::<Payload>(4);
    let mut audio = (
        requests,
        Retirer::new(Arc::clone(&queue), 2),
        Module {
            current: Shared::new(vec![0u8; 64]),
        },
    );
    submit.send(Payload::new(vec![1u8; 64])).unwrap();
    submit.send(Payload::new(7u32)).unwrap();

    // Adopt the new value and retire the replaced one.
    on_audio(&mut audio, |(requests, retirer, module)| {
        let payload = requests.try_recv().unwrap();
        assert!(payload.is::<Vec<u8>>() && !payload.is::<u32>());
        module.apply(payload, retirer);
    });
    assert_eq!(audio.2.current[0], 1);

    // A wrong-type payload comes back from downcast and retires.
    on_audio(&mut audio, |(requests, retirer, _)| {
        let payload = requests.try_recv().unwrap();
        let Err(refused) = payload.downcast::<Vec<u8>>() else {
            panic!("downcast to the wrong type");
        };
        assert!(refused.is::<u32>());
        retirer.retire(refused);
    });

    // A clone of the module's value retires by a reference-count move.
    on_audio(&mut audio, |(_, retirer, module)| {
        let clone = module.current.clone();
        retirer.retire(clone);
    });
    assert_eq!(audio.1.held(), 0);
    assert_eq!(queue.drain(), 3);
    assert_eq!(audio.2.current[0], 1);
}

#[test]
fn retired_values_drop_on_the_draining_thread() {
    let ledger = Ledger::default();
    let queue = RetireQueue::with_capacity(8);
    let mut retirer = Retirer::new(Arc::clone(&queue), 2);
    let owned = Box::new(ledger.value(1));
    let shared = Shared::new(ledger.value(2));
    let last = Shared::new(ledger.value(3));
    let clone = last.clone();
    let payload = Payload::new(ledger.value(4));

    let ((), audio) = on_audio(&mut retirer, |retirer| {
        retirer.retire(owned);
        retirer.retire(shared);
        retirer.retire(Retired::from(payload));
        retirer.retire(last);
    });
    assert!(ledger.drops().is_empty(), "retired value dropped early");

    // The audio side holds the last clone of value 3; it retires that too.
    let ((), audio_again) = on_audio(&mut retirer, |retirer| retirer.retire(clone));
    let (freed, control) = drain_on_control(&queue);
    assert_eq!(freed, 5);
    assert!(control != audio && control != audio_again);
    assert_eq!(ledger.dropped_ids(), [1, 2, 3, 4]);
    assert!(ledger
        .drops()
        .iter()
        .all(|&(_, dropper)| dropper == control));
}

#[test]
fn a_full_retire_path_holds_and_drops_every_value_once_off_the_audio_thread() {
    let ledger = Ledger::default();
    let queue = RetireQueue::with_capacity(1);
    let mut retirer = Retirer::new(Arc::clone(&queue), 2);
    let [one, two, three] = [1, 2, 3].map(|id| Box::new(ledger.value(id)));

    // The queue takes value 1; values 2 and 3 are held.
    let ((), audio) = on_audio(&mut retirer, |retirer| {
        retirer.retire(one);
        assert!(retirer.has_room(2));
        retirer.retire(two);
        assert!(retirer.has_room(1) && !retirer.has_room(2));
        retirer.retire(three);
        assert!(!retirer.has_room(1));
        // Nothing drained: flushing moves nothing.
        retirer.flush();
        assert_eq!(retirer.held(), 2);
    });
    let mut audio_threads = vec![audio];
    assert!(ledger.drops().is_empty(), "audio side dropped");

    // Each drain makes room for one held retirement.
    for (drained, held) in [(1, 1), (2, 0)] {
        assert_eq!(queue.drain(), 1);
        assert_eq!(ledger.drops().len(), drained);
        let ((), audio) = on_audio(&mut retirer, |retirer| {
            retirer.flush();
            assert_eq!(retirer.held(), held);
        });
        audio_threads.push(audio);
        assert_eq!(ledger.drops().len(), drained, "audio side dropped");
    }
    assert_eq!(queue.drain(), 1);
    assert_eq!(queue.drain(), 0);
    assert_eq!(ledger.dropped_ids(), [1, 2, 3]);
    assert!(ledger
        .drops()
        .iter()
        .all(|(_, dropper)| !audio_threads.contains(dropper)));
    assert_eq!(retirer.leaked(), 0);
}

#[test]
fn a_saturated_retire_path_defers_requests_in_order() {
    let ledger = Ledger::default();
    let control = thread::current().id();
    let queue = RetireQueue::with_capacity(1);
    let (sender, requests) = mpsc::sync_channel::<Payload>(2);
    let submit = |id| sender.try_send(Payload::new(ledger.value(id)));
    let mut audio = (
        requests,
        Retirer::new(Arc::clone(&queue), MAX_RETIRES_PER_REQUEST),
        Module {
            current: Shared::new(ledger.value(0)),
        },
    );
    let mut block = || {
        on_audio(&mut audio, |(r, retirer, module)| {
            run_block(r, retirer, module)
        })
        .0
    };

    // Value 0 takes the only queue slot, value 1 is held.
    submit(1).unwrap();
    submit(2).unwrap();
    assert_eq!(block(), 2);

    // Without a drain there is no room for a whole request: requests wait,
    // and the full request queue refuses the next submit.
    submit(3).unwrap();
    submit(4).unwrap();
    assert_eq!(block(), 0);
    assert!(matches!(submit(5), Err(TrySendError::Full(_))));
    assert_eq!(ledger.dropped_ids(), [5]);

    // Each drain frees one slot, and requests apply in order.
    let mut applied = 0;
    while applied < 2 {
        queue.drain();
        applied += block();
    }
    queue.drain();
    assert_eq!(block(), 0);
    queue.drain();
    assert_eq!(audio.2.current.id, 4);
    assert_eq!(audio.1.held(), 0);
    assert_eq!(ledger.dropped_ids(), [0, 1, 2, 3, 5]);
    assert!(ledger
        .drops()
        .iter()
        .all(|&(_, dropper)| dropper == control));
}

#[test]
fn submit_apply_and_retire_across_threads_including_the_last_arc() {
    let ledger = Ledger::default();
    let queue = RetireQueue::with_capacity(4);
    let (submit, requests) = mpsc::sync_channel::<Payload>(4);
    let (step, steps) = mpsc::sync_channel::<()>(0);
    let (report, reports) = mpsc::sync_channel::<(usize, ThreadId)>(0);
    let control = thread::current().id();

    let mut retirer = Retirer::new(Arc::clone(&queue), MAX_RETIRES_PER_REQUEST);
    let mut module = Module {
        current: Shared::new(ledger.value(0)),
    };
    let audio = thread::spawn(move || {
        while steps.recv().is_ok() {
            let (applied, allocs, frees) = allocator_events(|| {
                let _scope = AudioThreadScope::enter();
                run_block(&requests, &mut retirer, &mut module)
            });
            assert_eq!((allocs, frees), (0, 0));
            report.send((applied, thread::current().id())).unwrap();
        }
        (retirer, module)
    });
    let step_block = || {
        step.send(()).unwrap();
        reports.recv().unwrap()
    };

    // The control side keeps a handle on value 1 while it is applied.
    let kept = Shared::new(ledger.value(1));
    submit.send(Payload::from(kept.clone())).unwrap();
    let (applied, audio_id) = step_block();
    assert_eq!(applied, 1);
    assert_ne!(audio_id, control);
    assert!(ledger.drops().is_empty());
    assert_eq!(queue.drain(), 1);
    assert_eq!(ledger.drops(), [(0, control)]);

    // Dropping the control side's handle leaves the audio side the last one.
    drop(kept);
    assert_eq!(ledger.drops().len(), 1);
    submit.send(Payload::new(ledger.value(2))).unwrap();
    submit.send(Payload::new("wrong type")).unwrap();
    assert_eq!(step_block().0, 2);
    assert_eq!(ledger.drops().len(), 1, "the last Arc dropped on audio");
    assert_eq!(queue.drain(), 2);

    drop(step);
    let (retirer, module) = audio.join().unwrap();
    assert_eq!(module.current.id, 2);
    drop((retirer, module));
    assert_eq!(ledger.dropped_ids(), [0, 1, 2]);
    assert!(ledger
        .drops()
        .iter()
        .all(|&(_, dropper)| dropper == control));
}

#[test]
fn dropping_a_retirer_sends_held_values_on() {
    let ledger = Ledger::default();
    let queue = RetireQueue::with_capacity(1);
    let mut retirer = Retirer::new(Arc::clone(&queue), 1);
    retirer.retire(Box::new(ledger.value(1)));
    retirer.retire(Box::new(ledger.value(2)));
    assert_eq!(queue.drain(), 1);
    drop(retirer);
    assert_eq!(ledger.dropped_ids(), [1]);
    assert_eq!(queue.drain(), 1);
    assert_eq!(ledger.dropped_ids(), [1, 2]);
}

#[test]
fn the_reclaimer_drains_its_payload_queue() {
    use crate::invention::graph::Publication;
    use crate::invention::publish::Reclaimer;

    let ledger = Ledger::default();
    let retired = crate::spsc::Ring::<Box<Publication>>::with_capacity(1);
    let queue = RetireQueue::with_capacity(4);
    let reclaimer = Reclaimer::new(retired, Arc::clone(&queue));
    let mut retirer = Retirer::new(queue, RETIRE_HOLD);
    let value = Shared::new(ledger.value(1));
    let ((), audio) = on_audio(&mut retirer, |retirer| retirer.retire(value));
    assert!(ledger.drops().is_empty());
    assert_eq!(reclaimer.reclaim(), 0);
    let drops = ledger.drops();
    assert_eq!(drops.len(), 1);
    assert_ne!(drops[0].1, audio);
}

#[test]
fn the_producer_never_waits_for_a_stalled_consumer() {
    let ledger = Ledger::default();
    let queue = RetireQueue::with_capacity(1);
    let mut retirer = Retirer::new(Arc::clone(&queue), 2);
    let [one, two, three] = [1, 2, 3].map(|id| Box::new(ledger.value(id)));
    retirer.retire(one);
    let mut audio_threads = Vec::new();

    // The consumer has moved value 1 out but not yet released its slot: the
    // audio side finds the queue full and holds, without waiting.
    let popped = queue.pop_paused(|| {
        let ((), audio) = on_audio(&mut retirer, |retirer| {
            retirer.retire(two);
            retirer.flush();
            retirer.retire(three);
            assert_eq!(retirer.held(), 2);
        });
        audio_threads.push(audio);
    });
    drop(popped);
    assert_eq!(ledger.dropped_ids(), [1]);

    for (drained, held) in [(2, 1), (3, 0)] {
        let ((), audio) = on_audio(&mut retirer, |retirer| {
            retirer.flush();
            assert_eq!(retirer.held(), held);
        });
        audio_threads.push(audio);
        assert_eq!(queue.drain(), 1);
        assert_eq!(ledger.drops().len(), drained);
    }
    assert_eq!(ledger.dropped_ids(), [1, 2, 3]);
    assert!(ledger
        .drops()
        .iter()
        .all(|(_, dropper)| !audio_threads.contains(dropper)));
}

#[test]
#[should_panic(expected = "already has a producer")]
fn a_second_retirer_on_a_claimed_queue_panics() {
    let queue = RetireQueue::with_capacity(1);
    let _first = Retirer::new(Arc::clone(&queue), 1);
    let _second = Retirer::new(queue, 1);
}

#[test]
fn a_queue_is_claimed_again_after_its_retirer_drops_and_frees_leftovers() {
    let ledger = Ledger::default();
    let queue = RetireQueue::with_capacity(4);
    let mut first = Retirer::new(Arc::clone(&queue), 1);
    first.retire(Box::new(ledger.value(1)));
    drop(first);

    // A later retirer, on another thread, continues from the first's tail.
    let second_queue = Arc::clone(&queue);
    let value = Box::new(ledger.value(2));
    thread::spawn(move || Retirer::new(second_queue, 1).retire(value))
        .join()
        .unwrap();
    assert!(ledger.drops().is_empty());

    // Dropping the queue frees what it still holds.
    drop(queue);
    assert_eq!(ledger.dropped_ids(), [1, 2]);
}

#[cfg(debug_assertions)]
mod debug_checks {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use super::super::{Payload, RetireQueue, Retirer, Shared};
    use super::Retired;
    use crate::audio_thread::{on_audio_thread, AudioThreadScope};

    /// Drops `value` inside an audio-thread scope; returns the panic message.
    fn drop_on_audio<T>(value: T) -> String {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _scope = AudioThreadScope::enter();
            drop(value);
        }))
        .unwrap_err();
        assert!(!on_audio_thread(), "the scope did not restore on unwind");
        panic.downcast::<String>().map(|message| *message).unwrap()
    }

    #[test]
    fn dropping_on_the_audio_thread_panics() {
        let message = drop_on_audio(Payload::new(1u32));
        assert_eq!(
            message,
            "a Payload was dropped on the audio thread: retire it instead"
        );
        assert!(drop_on_audio(Shared::new(1u32)).starts_with("a Shared value"));
        assert!(drop_on_audio(Retired::from(Box::new(1u32))).starts_with("a Retired value"));

        // Outside the scope they drop quietly.
        drop(Payload::new(1u32));
        drop(Shared::new(1u32));
        drop(Retired::from(Box::new(1u32)));
    }

    #[test]
    fn converting_inside_the_scope_does_not_panic() {
        let payload = Payload::new(1u32);
        let other = Payload::new(1u32);
        let scope = AudioThreadScope::enter();
        let shared = payload.downcast::<u32>().unwrap();
        let payload = Payload::from(shared.clone());
        let retired = [Retired::from(shared), Retired::from(payload)];
        let Err(refused) = other.downcast::<u8>() else {
            panic!("downcast to the wrong type");
        };
        drop(scope);
        drop((retired, refused));
    }

    #[test]
    fn scopes_nest_and_restore() {
        assert!(!on_audio_thread());
        let outer = AudioThreadScope::enter();
        let inner = AudioThreadScope::enter();
        drop(inner);
        assert!(on_audio_thread());
        drop(outer);
        assert!(!on_audio_thread());
    }

    #[test]
    #[should_panic(expected = "check has_room first")]
    fn retiring_past_a_full_retirer_panics() {
        let queue = RetireQueue::with_capacity(1);
        let mut retirer = Retirer::new(queue, 1);
        retirer.retire(Box::new(1u32));
        retirer.retire(Box::new(2u32));
        assert!(!retirer.has_room(1));
        retirer.retire(Box::new(3u32));
    }
}
