//! Watches on their own, on a scripted timeline: when each applies, how
//! long the segment before it may run, a beat passed inside a segment
//! (applied late), resets, refusals, and remapping across an install.

use super::*;
use crate::alloc_counter::allocator_events;
use crate::control_request::pending::PendingStore;
use crate::control_request::timeline::first_sample_reaching;
use crate::control_request::{
    outcome_channel, BeatTime, ControlIndex, OutcomeReceiver, RequestId, RtValue, BEFORE_START,
};

const INSTALLED: u64 = 1;

/// A timeline moving `rate` beats a sample from `position`.
struct Scripted {
    position: f64,
    rate: f64,
    beats_before: u64,
}

impl Timeline for Scripted {
    fn position(&self) -> f64 {
        self.position
    }

    fn position_after(&self, samples: u64) -> f64 {
        self.position + samples as f64 * self.rate
    }

    fn samples_until(&self, beat: f64) -> Option<u64> {
        first_sample_reaching(|k| self.position_after(k), beat, 2.0)
    }

    fn beats_before(&self) -> u64 {
        self.beats_before
    }

    fn latch(&mut self) {}
}

/// A scripted clock at module 0, recording the targets it applies to and
/// the clocks it latches.
struct Host {
    clock: Scripted,
    applied: Vec<usize>,
    latched: Vec<usize>,
}

impl Host {
    fn at(position: f64, rate: f64) -> Self {
        Self {
            clock: Scripted {
                position,
                rate,
                beats_before: 0,
            },
            applied: Vec::with_capacity(8),
            latched: Vec::with_capacity(8),
        }
    }

    /// Plays `samples` samples.
    fn play(&mut self, samples: u64) {
        self.clock.position = self.clock.position_after(samples);
    }
}

impl BeatHost for Host {
    fn timeline(&self, module_idx: usize) -> Option<&dyn Timeline> {
        (module_idx == 0).then_some(&self.clock as &dyn Timeline)
    }

    fn latch(&mut self, module_idx: usize) {
        self.latched.push(module_idx);
    }

    /// A request to the clock itself resets it.
    fn apply(
        &mut self,
        target: &ControlTarget,
        _value: RequestValue,
        _retirer: &mut Retirer,
    ) -> Result<(), Refusal> {
        self.applied.push(target.module_idx);
        if target.module_idx == 0 {
            let begun = (self.clock.position.floor() + 1.0) as u64;
            self.clock.beats_before += begun;
            self.clock.position = BEFORE_START;
        }
        Ok(())
    }
}

fn store() -> (PendingStore, OutcomeReceiver) {
    let queue = crate::payload::RetireQueue::with_capacity(4);
    let (sender, outcomes) = outcome_channel(64);
    (
        PendingStore::new(4, Retirer::new(queue, 8), sender),
        outcomes,
    )
}

/// Request `id` to module `module_idx`, `beats` after its clock's position.
fn request(id: u64, module_idx: usize, beats: f32) -> Request {
    let target = ControlTarget {
        generation: INSTALLED,
        module_idx,
        control: ControlIndex(0),
    };
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(1.0)));
    request.when = When::Beat(BeatTime {
        clock: 0,
        spec: BeatSpec::After(beats),
    });
    request.id = RequestId(id);
    request
}

fn log(outcomes: &OutcomeReceiver) -> Vec<(u64, Outcome)> {
    std::iter::from_fn(|| outcomes.try_recv())
        .map(|(id, outcome)| (id.0, outcome))
        .collect()
}

#[test]
fn a_beat_no_clock_reaches_is_refused() {
    for beats in [-0.5, f32::NAN, f32::INFINITY] {
        let request = request(1, 1, beats);
        assert_eq!(Watches::check(&request), Err(Refusal::Invalid), "{beats}");
    }
    assert_eq!(Watches::check(&request(1, 1, 0.0)), Ok(()));
    let mut timed = request(1, 1, 1.0);
    timed.when = When::AtSample(3);
    assert_eq!(Watches::check(&timed), Err(Refusal::Unsupported));
}

#[test]
fn a_watch_ends_each_segment_before_its_beat_and_applies_on_it() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    // From 0.5, at 1/128 a beat a sample: beat 2.5 is the 256th sample,
    // beat 1 the 64th.
    watches.insert(request(1, 1, 2.0), 0, &mut store.outcomes);
    watches.insert(request(2, 2, 0.5), 0, &mut store.outcomes);
    let mut host = Host::at(0.5, 1.0 / 128.0);
    let mut now = 1_000;
    let mut segments = Vec::new();
    while !watches.is_empty() {
        let (_, wait) = watches.pass(now, INSTALLED, &mut host, &mut store.outcomes);
        segments.extend(wait);
        host.play(wait.unwrap_or(1));
        now += wait.unwrap_or(1);
    }
    assert_eq!(segments, [63, 192]);
    assert_eq!(host.applied, [2, 1]);
    let applied = |at| Outcome::Applied { at };
    assert_eq!(log(&outcomes), [(2, applied(1_063)), (1, applied(1_255))]);
}

#[test]
fn a_pass_neither_allocates_nor_frees() {
    let (mut store, _outcomes) = store();
    let mut watches = Watches::new(4);
    watches.insert(request(1, 1, 2.0), 0, &mut store.outcomes);
    watches.insert(request(2, 1, 0.0), 0, &mut store.outcomes);
    let mut host = Host::at(0.0, 0.01);
    let ((), allocs, frees) = allocator_events(|| {
        watches.pass(10, INSTALLED, &mut host, &mut store.outcomes);
    });
    assert_eq!((allocs, frees, watches.len()), (0, 0, 1));
}

#[test]
fn a_beat_passed_inside_a_segment_applies_late_and_says_when_it_was_due() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    watches.insert(request(7, 1, 4.0), 0, &mut store.outcomes);
    let mut host = Host::at(1.0, 1.0 / 128.0);
    // Beat 5 is 512 samples away at this rate: the segment ends before it.
    let pass = watches.pass(1_000, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (false, Some(511)));
    // The tempo doubled inside the segment: the 51 samples before this one
    // reached beat 5, the first of them at 1460.
    host.clock.position = 5.0 + 50.0 / 64.0;
    host.clock.rate = 1.0 / 64.0;
    let pass = watches.pass(1_511, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (true, None));
    let late = Outcome::AppliedLate {
        at: 1_511,
        due: 1_460,
    };
    assert_eq!(log(&outcomes), [(7, late)]);
    assert!(watches.is_empty());
}

#[test]
fn a_reset_counts_as_reaching_the_next_whole_beat() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    watches.insert(request(1, 1, 4.0), 0, &mut store.outcomes);
    let mut host = Host::at(0.25, 1.0 / 128.0);
    watches.pass(0, INSTALLED, &mut host, &mut store.outcomes);
    // Reset at 1.5: beats 0 and 1 begun, and the reset starts the third,
    // so beat 4.25 of the count is beat 2.25 of the clock now.
    host.clock = Scripted {
        position: BEFORE_START,
        rate: 1.0 / 128.0,
        beats_before: 2,
    };
    let pass = watches.pass(500, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (false, Some(287)));
    assert!(log(&outcomes).is_empty());
}

#[test]
fn a_reset_past_the_beat_reaches_it_on_time() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    watches.insert(request(1, 1, 0.5), 0, &mut store.outcomes);
    let mut host = Host::at(2.2, 1.0 / 128.0);
    watches.pass(0, INSTALLED, &mut host, &mut store.outcomes);
    // Reset at 2.5, short of beat 2.7: it starts beat 3 of the count.
    host.clock = Scripted {
        position: BEFORE_START,
        rate: 1.0 / 128.0,
        beats_before: 3,
    };
    let pass = watches.pass(38, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (true, None));
    assert_eq!(log(&outcomes), [(1, Outcome::Applied { at: 38 })]);
}

#[test]
fn spans_count_from_the_pass_start_whatever_applies_first() {
    for reset_first in [true, false] {
        let (mut store, outcomes) = store();
        let mut watches = Watches::new(4);
        // A reset now and a level a quarter beat on, from 0.5: the reset
        // starts beat 1, past 0.75, so the level applies with it.
        let (reset, level) = (request(1, 0, 0.0), request(2, 1, 0.25));
        let order = if reset_first {
            [reset, level]
        } else {
            [level, reset]
        };
        for request in order {
            watches.insert(request, 0, &mut store.outcomes);
        }
        let mut host = Host::at(0.5, 1.0 / 128.0);
        while watches.pass(9, INSTALLED, &mut host, &mut store.outcomes).0 {}
        let mut got = log(&outcomes);
        got.sort_by_key(|(id, _)| *id);
        let applied = Outcome::Applied { at: 9 };
        assert_eq!(got, [(1, applied), (2, applied)], "{reset_first}");
        assert_eq!(host.latched, [0, 0], "each latches its clock");
    }
}

#[test]
fn a_watch_is_refused_when_its_ttl_runs_out_or_its_clock_keeps_no_timeline() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    let mut expiring = request(1, 1, 4.0);
    expiring.expires = Some(99);
    watches.insert(expiring, 0, &mut store.outcomes);
    let mut on_no_clock = request(2, 1, 4.0);
    on_no_clock.when = When::Beat(BeatTime {
        clock: 5,
        spec: BeatSpec::After(4.0),
    });
    watches.insert(on_no_clock, 5, &mut store.outcomes);
    let mut host = Host::at(0.0, 0.001);
    watches.pass(99, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(log(&outcomes), [(2, Outcome::Refused(Refusal::NoTimeline))]);
    watches.pass(100, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(log(&outcomes), [(1, Outcome::Refused(Refusal::Expired))]);
    assert!(watches.is_empty());
}

#[test]
fn a_full_store_refuses_another_watch() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(1);
    watches.insert(request(1, 1, 4.0), 0, &mut store.outcomes);
    assert!(watches.is_full());
    watches.insert(request(2, 1, 4.0), 0, &mut store.outcomes);
    assert_eq!(
        log(&outcomes),
        [(2, Outcome::Refused(Refusal::PendingFull))]
    );
}

#[test]
fn an_install_maps_target_and_clock_or_refuses_what_went_away() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    let older = |id, module_idx, clock: u32| {
        let mut request = request(id, module_idx, 1.0);
        request.target.generation = INSTALLED - 1;
        (request, clock as usize)
    };
    for (request, clock) in [older(1, 1, 0), older(2, 9, 0), older(3, 1, 9)] {
        watches.insert(request, clock, &mut store.outcomes);
    }
    let mut held = request(4, 7, 1.0);
    held.target.generation = INSTALLED + 1;
    watches.insert(held, 0, &mut store.outcomes);
    // Module 9 went away; the others moved up by one.
    watches.remap(INSTALLED, &mut store.outcomes, |generation, module_idx| {
        assert_eq!(generation, INSTALLED - 1);
        (module_idx != 9).then_some(module_idx + 1)
    });
    assert_eq!(
        log(&outcomes),
        [
            (2, Outcome::Refused(Refusal::TargetGone)),
            (3, Outcome::Refused(Refusal::TimelineGone)),
        ]
    );
    assert_eq!(watches.clocks(INSTALLED).collect::<Vec<_>>(), [1]);
    assert_eq!(watches.clocks(INSTALLED + 1).collect::<Vec<_>>(), [0]);
}

/// Request `id` to module 1 at a grid on clock 0.
fn on_grid(id: u64, every: f32, offset: f32, repeat: bool) -> Request {
    let mut request = request(id, 1, 0.0);
    request.when = When::Beat(BeatTime {
        clock: 0,
        spec: BeatSpec::Grid {
            every,
            offset,
            repeat,
        },
    });
    request
}

#[test]
fn the_next_grid_beat_is_strictly_past_the_position() {
    assert_eq!(grid_after(BEFORE_START, 4.0, 0.0), 0.0);
    assert_eq!(grid_after(0.0, 4.0, 0.0), 4.0);
    assert_eq!(grid_after(3.99, 4.0, 0.0), 4.0);
    assert_eq!(grid_after(4.0, 4.0, 1.5), 5.5);
    assert_eq!(grid_after(6.0, 4.0, 1.5), 9.5);
    assert_eq!(grid_after(6.0, 2.0, -0.5), 7.5);
    assert_eq!(grid_after(0.1, 0.75, 0.0), 0.75);
    for from in (0..2_000).map(|n| n as f64 * 0.0371) {
        let next = grid_after(from, 0.3, 0.1);
        assert!(next > from && next - 0.3 <= from, "{from} -> {next}");
    }
}

#[test]
fn a_grid_no_clock_reaches_or_a_repeating_payload_is_refused() {
    let invalid = [
        (0.0, 0.0),
        (-1.0, 0.0),
        (f32::INFINITY, 0.0),
        (1.0, f32::NAN),
    ];
    for (every, offset) in invalid {
        let request = on_grid(1, every, offset, false);
        assert_eq!(
            Watches::check(&request),
            Err(Refusal::Invalid),
            "{every} {offset}"
        );
    }
    assert_eq!(Watches::check(&on_grid(1, 0.25, -3.0, true)), Ok(()));
    // A repeat finer than a 256th note is refused; once, it is not.
    let step = MIN_REPEAT_STEP;
    assert_eq!(Watches::check(&on_grid(1, step, 0.0, true)), Ok(()));
    let fine = on_grid(1, step / 2.0, 0.0, true);
    assert_eq!(Watches::check(&fine), Err(Refusal::Invalid));
    assert_eq!(Watches::check(&on_grid(1, step / 2.0, 0.0, false)), Ok(()));
    let mut payload = on_grid(1, 1.0, 0.0, true);
    payload.value = RequestValue::Payload(crate::payload::Payload::new(vec![0.0f32; 4]));
    assert_eq!(Watches::check(&payload), Err(Refusal::Unsupported));
}

#[test]
fn a_repeating_grid_applies_once_a_sample_and_stays() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    watches.insert(on_grid(7, 1.0, 0.0, true), 0, &mut store.outcomes);
    // A clock covering two beats a sample still applies once a sample.
    let mut host = Host::at(0.5, 2.0);
    let pass = watches.pass(10, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (true, None));
    let pass = watches.pass(10, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (false, Some(1)));
    assert_eq!(log(&outcomes), [(7, Outcome::Applied { at: 10 })]);
    assert_eq!(watches.len(), 1);
    // It applies again at the next sample, settled already: no outcome.
    host.play(1);
    let pass = watches.pass(11, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (true, None));
    assert_eq!(host.applied, [1, 1]);
    assert!(log(&outcomes).is_empty());
}

#[test]
fn a_repeat_that_applied_ends_silently_and_one_that_did_not_is_refused() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    for (id, generation) in [(1, INSTALLED), (2, INSTALLED - 1)] {
        let mut request = on_grid(id, 1.0, 0.0, true);
        request.expires = Some(50);
        request.target.generation = generation;
        watches.insert(request, 0, &mut store.outcomes);
    }
    let mut host = Host::at(0.9, 1.0 / 8.0);
    watches.pass(10, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(log(&outcomes), [(1, Outcome::Applied { at: 10 })]);
    // Its ttl runs out: it applied, so it ends without another outcome.
    watches.pass(51, INSTALLED, &mut host, &mut store.outcomes);
    assert!(log(&outcomes).is_empty());
    // The other never applied: its clock going away refuses it.
    watches.remap(INSTALLED, &mut store.outcomes, |_, _| None);
    let gone = Outcome::Refused(Refusal::TargetGone);
    assert_eq!(log(&outcomes), [(2, gone)]);
    assert!(watches.is_empty());
}

#[test]
fn a_grid_beat_passed_inside_a_segment_applies_late_once() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    watches.insert(on_grid(7, 2.0, 1.0, true), 0, &mut store.outcomes);
    let mut host = Host::at(0.0, 1.0 / 128.0);
    // Beat 1 is 128 samples away.
    let pass = watches.pass(0, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (false, Some(127)));
    // The clock raced past beats 1 and 3 inside the segment: beat 1
    // applies late, once, and the grid goes on from beat 5.
    host.clock.position = 3.5;
    let pass = watches.pass(127, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (true, None));
    let pass = watches.pass(127, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (false, Some(191)));
    let [(7, Outcome::AppliedLate { at: 127, due })] = log(&outcomes)[..] else {
        panic!("not applied late once");
    };
    assert!(due < 127);
}

#[test]
fn a_grid_follows_the_clock_s_new_beat_0_after_a_reset() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    watches.insert(on_grid(7, 4.0, 0.0, false), 0, &mut store.outcomes);
    let mut host = Host::at(2.0, 1.0 / 128.0);
    let pass = watches.pass(0, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (false, Some(255)));
    // Reset at 3: the next multiple of 4 is the new beat 0, at once.
    host.clock = Scripted {
        position: BEFORE_START,
        rate: 1.0 / 128.0,
        beats_before: 4,
    };
    let pass = watches.pass(128, INSTALLED, &mut host, &mut store.outcomes);
    assert_eq!(pass, (true, None));
    assert_eq!(log(&outcomes), [(7, Outcome::Applied { at: 128 })]);
}
