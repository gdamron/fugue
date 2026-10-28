//! The condition table on its own, over scripted signals: the exact sample
//! each kind of signal part is met at, AND across parts and segments, a
//! condition known only after its target computed (applied late), ttl,
//! the cap, remapping, and no allocation while evaluating.

use std::sync::Arc;

use super::*;
use crate::alloc_counter::allocator_events;
use crate::control_request::hold::Holds;
use crate::control_request::pending::PendingStore;
use crate::control_request::{
    outcome_channel, request_channel, ControlIndex, Edge, OutcomeReceiver, RequestId, RtValue,
};

pub(super) const INSTALLED: u64 = 1;
/// The module whose control the requests write.
pub(super) const TARGET: usize = 9;

/// Signals by module (port 0 each), and how far each is computed in the
/// current segment.
pub(super) struct Host {
    signals: Vec<Vec<f32>>,
    start: u64,
    computed: Vec<u64>,
    /// The sample the target computes next, where requests apply.
    pub(super) now: u64,
    pub(super) applied: Vec<(usize, u64)>,
    /// Control 0 of each module.
    controls: Vec<f32>,
    /// How many requests had applied each time the late events applied
    /// were finished.
    pub(super) finished: Vec<usize>,
    /// Holds on the target's controls, weighed as the graph weighs them,
    /// when set.
    pub(super) holds: Option<Holds>,
}

impl Host {
    pub(super) fn new(signals: Vec<Vec<f32>>) -> Self {
        let computed = vec![0; signals.len()];
        Self {
            signals,
            start: 0,
            computed,
            now: 0,
            applied: Vec::with_capacity(8),
            controls: vec![0.0; 2],
            finished: Vec::with_capacity(8),
            holds: None,
        }
    }

    /// Starts a segment at `start`, nothing computed yet.
    pub(super) fn segment(&mut self, start: u64) {
        self.start = start;
        self.now = start;
        self.computed.fill(start);
    }

    /// Computes module `module` up to sample `to`.
    pub(super) fn compute(&mut self, module: usize, to: u64) {
        self.computed[module] = to;
    }
}

impl ConditionHost for Host {
    fn output(&self, module: usize, port: usize) -> Option<(u64, &[f32])> {
        let signal = self.signals.get(module).filter(|_| port == 0)?;
        let (from, to) = (self.start as usize, self.computed[module] as usize);
        Some((self.start, &signal[from..to]))
    }

    fn last_output(&self, module: usize, port: usize) -> Option<f32> {
        let signal = self.signals.get(module).filter(|_| port == 0)?;
        Some(match self.start {
            0 => 0.0,
            start => signal[start as usize - 1],
        })
    }

    fn control(&self, module: usize, control: ControlIndex) -> Option<f32> {
        self.controls
            .get(module)
            .filter(|_| control.0 == 0)
            .copied()
    }

    fn apply(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        _value: RequestValue,
        _outcomes: &mut Outcomes,
    ) -> Result<(), Outcome> {
        if let (Some(holds), Some(standing)) = (&self.holds, standing) {
            holds.admits(target.control, standing)?;
            holds.take(target.control, standing);
        }
        self.applied.push((target.module_idx, self.now));
        Ok(())
    }

    fn finish_events(&mut self) {
        self.finished.push(self.applied.len());
    }
}

/// A gate high for two samples at each of `onsets`, `len` samples long.
fn gate(onsets: &[usize], len: usize) -> Vec<f32> {
    let mut signal = vec![0.0; len];
    for &onset in onsets {
        signal[onset..onset + 2].fill(1.0);
    }
    signal
}

pub(super) fn store() -> (PendingStore, OutcomeReceiver) {
    store_for(64)
}

/// A store whose outcome channel holds `outcomes` before it drops any.
pub(super) fn store_for(outcomes: usize) -> (PendingStore, OutcomeReceiver) {
    let queue = crate::payload::RetireQueue::with_capacity(4);
    let (sender, outcomes) = outcome_channel(outcomes);
    let store = PendingStore::new(4, Retirer::new(queue, 8), sender);
    (store, outcomes)
}

/// Request `id` for the target, when `parts` are all met.
pub(super) fn request(id: u64, parts: &[Predicate]) -> Request {
    let target = ControlTarget {
        generation: INSTALLED,
        module_idx: TARGET,
        control: ControlIndex(0),
    };
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(1.0)));
    request.when = When::Condition(Condition::all(parts).unwrap());
    request.id = RequestId(id);
    request
}

/// A table of `capacity` and its slots.
pub(super) fn table(capacity: usize) -> (Conditions, Arc<WatchSlots>) {
    let slots = Arc::new(WatchSlots::default());
    (Conditions::new(capacity, Arc::clone(&slots)), slots)
}

/// Inserts `request` as the drain would, its slot taken at submission.
pub(super) fn insert(
    table: &mut Conditions,
    slots: &WatchSlots,
    request: Request,
    store: &mut PendingStore,
) {
    slots.take();
    assert_eq!(Conditions::check(&request), Ok(()));
    table.insert(request, &mut store.outcomes);
}

pub(super) fn log(outcomes: &OutcomeReceiver) -> Vec<(u64, Outcome)> {
    std::iter::from_fn(|| outcomes.try_recv())
        .map(|(id, outcome, _)| (id.0, outcome))
        .collect()
}

/// Plays one segment of `frames` from `start` with every source computed
/// before the target: reads, then applies at each sample the target is
/// split at. Returns the samples it split the target at.
pub(super) fn play_ordered(
    conditions: &mut Conditions,
    host: &mut Host,
    store: &mut PendingStore,
    start: u64,
    frames: u64,
) -> Vec<u64> {
    host.segment(start);
    conditions.start(start, INSTALLED, host, &mut store.outcomes);
    for module in 0..host.signals.len() {
        host.compute(module, start + frames);
    }
    conditions.read(INSTALLED, Some(TARGET), host, &store.outcomes.chains);
    let mut splits = Vec::new();
    let mut at = start;
    while let Some(next) = apply_all(conditions, host, store, at) {
        if next >= start + frames {
            break;
        }
        splits.push(next);
        at = next;
        host.now = next;
    }
    conditions.read(INSTALLED, None, host, &store.outcomes.chains);
    splits
}

/// Applies what is due on the target at `at`, and what that meets in
/// turn, as the graph does.
pub(super) fn apply_all(
    conditions: &mut Conditions,
    host: &mut Host,
    store: &mut PendingStore,
    at: u64,
) -> Option<u64> {
    loop {
        let waiting = conditions.len();
        let next = conditions.apply_due(INSTALLED, Some(TARGET), at, host, store);
        if conditions.len() == waiting {
            return next;
        }
        conditions.chain(INSTALLED, &store.outcomes.chains);
    }
}

#[test]
fn a_condition_nothing_can_meet_is_refused() {
    let edges = |count| request(1, &[Predicate::edges(0, 0, count)]);
    assert_eq!(Conditions::check(&edges(0)), Err(Refusal::Invalid));
    assert_eq!(Conditions::check(&edges(1)), Ok(()));
    let nan = Predicate::Signal {
        module: 0,
        port: 0,
        edge: Edge::Rising,
        level: f32::NAN,
        count: 1,
    };
    assert_eq!(
        Conditions::check(&request(1, &[nan])),
        Err(Refusal::Invalid)
    );
    let mut timed = edges(1);
    timed.when = When::Now;
    assert_eq!(Conditions::check(&timed), Err(Refusal::Unsupported));
    let chained = request(1, &[Predicate::Applied(RequestId(3))]);
    assert_eq!(Conditions::check(&chained), Ok(()));
    let five = [Predicate::edges(0, 0, 1); 5];
    assert_eq!(Condition::all(&five), None);
    assert_eq!(Condition::all(&[]), None);
}

#[test]
fn the_nth_rising_edge_applies_on_its_exact_sample() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    // Gate onsets at 10, 20, 30, 40: the third is sample 30.
    let mut host = Host::new(vec![gate(&[10, 20, 30, 40], 64)]);
    insert(
        &mut conditions,
        &slots,
        request(1, &[Predicate::edges(0, 0, 3)]),
        &mut store,
    );
    let splits = play_ordered(&mut conditions, &mut host, &mut store, 0, 64);
    assert_eq!(splits, [30]);
    assert_eq!(host.applied, [(TARGET, 30)]);
    assert_eq!(log(&outcomes), [(1, Outcome::Applied { at: 30 })]);
    assert_eq!((conditions.len(), slots.taken()), (0, 0));
}

#[test]
fn a_gate_already_high_when_armed_is_not_an_edge_and_counting_spans_segments() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    // High at 15 and 16 (armed at 16, so not counted), then 40, 70, 100.
    let mut host = Host::new(vec![gate(&[15, 40, 70, 100], 128)]);
    play_ordered(&mut conditions, &mut host, &mut store, 0, 16);
    insert(
        &mut conditions,
        &slots,
        request(1, &[Predicate::edges(0, 0, 2)]),
        &mut store,
    );
    assert!(play_ordered(&mut conditions, &mut host, &mut store, 16, 48).is_empty());
    assert_eq!(
        play_ordered(&mut conditions, &mut host, &mut store, 64, 64),
        [70]
    );
    assert_eq!(log(&outcomes), [(1, Outcome::Applied { at: 70 })]);
}

#[test]
fn all_parts_are_met_at_the_last_ones_sample() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let falling = Predicate::Signal {
        module: 2,
        port: 0,
        edge: Edge::Falling,
        level: 0.25,
        count: 1,
    };
    let ramp_down = (0..64).map(|i| 1.0 - i as f32 / 32.0).collect();
    let mut host = Host::new(vec![gate(&[5], 64), gate(&[12, 33], 64), ramp_down]);
    // Edges at 5 and the second at 33; the ramp falls below 0.25 at 25.
    let parts = [
        Predicate::edges(0, 0, 1),
        Predicate::edges(1, 0, 2),
        falling,
    ];
    insert(&mut conditions, &slots, request(1, &parts), &mut store);
    insert(&mut conditions, &slots, request(2, &parts[..1]), &mut store);
    let four = [parts[0], parts[0], parts[2], parts[2]];
    insert(&mut conditions, &slots, request(3, &four), &mut store);
    let splits = play_ordered(&mut conditions, &mut host, &mut store, 0, 64);
    assert_eq!(splits, [5, 25, 33]);
    let applied = |at| Outcome::Applied { at };
    let expected = [(2, applied(5)), (3, applied(25)), (1, applied(33))];
    assert_eq!(log(&outcomes), expected);
}

#[test]
fn a_condition_read_only_after_its_target_computed_applies_late() {
    for event in [false, true] {
        let (mut store, outcomes) = store();
        let (mut conditions, slots) = table(4);
        let mut host = Host::new(vec![gate(&[30], 128)]);
        let mut late = request(1, &[Predicate::edges(0, 0, 1)]);
        late.event = event;
        insert(&mut conditions, &slots, late, &mut store);
        // The target computes first, then the source: nothing to apply yet.
        host.segment(0);
        conditions.start(0, INSTALLED, &host, &mut store.outcomes);
        conditions.read(INSTALLED, Some(TARGET), &host, &store.outcomes.chains);
        let next = conditions.apply_due(INSTALLED, Some(TARGET), 0, &mut host, &mut store);
        assert_eq!(next, None);
        host.compute(0, 64);
        conditions.read(INSTALLED, None, &host, &store.outcomes.chains);
        // Met at 30, it applies before the target's next sample, at 64.
        host.segment(64);
        conditions.start(64, INSTALLED, &host, &mut store.outcomes);
        conditions.apply_due(INSTALLED, Some(TARGET), 64, &mut host, &mut store);
        let late = Outcome::AppliedLate { at: 64, due: 30 };
        assert_eq!(log(&outcomes), [(1, late)]);
        // A late event finishes as it applies.
        assert_eq!(host.finished.len(), usize::from(event));
    }
}

#[test]
fn late_events_due_at_one_sample_each_finish_as_they_apply() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![gate(&[30], 128)]);
    let edge = [Predicate::edges(0, 0, 1)];
    for (id, event) in [(1, true), (2, true), (3, false)] {
        let mut late = request(id, &edge);
        late.event = event;
        insert(&mut conditions, &slots, late, &mut store);
    }
    host.segment(0);
    conditions.start(0, INSTALLED, &host, &mut store.outcomes);
    host.compute(0, 64);
    conditions.read(INSTALLED, None, &host, &store.outcomes.chains);
    host.segment(64);
    conditions.start(64, INSTALLED, &host, &mut store.outcomes);
    conditions.apply_due(INSTALLED, Some(TARGET), 64, &mut host, &mut store);
    let late = Outcome::AppliedLate { at: 64, due: 30 };
    // The value due beside them first, as in the pending store, so each
    // event reads it.
    assert_eq!(log(&outcomes), [(3, late), (1, late), (2, late)]);
    // As the pending store's and a replay's: each before the next applies.
    assert_eq!(host.finished, [2, 3]);
}

#[test]
fn a_ttl_refuses_a_condition_not_met_in_time() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![gate(&[30, 90], 128)]);
    let edge = [Predicate::edges(0, 0, 1)];
    let mut waits = request(1, &edge);
    waits.expires = Some(50);
    let mut late = request(2, &[Predicate::edges(0, 0, 2)]);
    late.expires = Some(80);
    insert(&mut conditions, &slots, waits, &mut store);
    insert(&mut conditions, &slots, late, &mut store);
    play_ordered(&mut conditions, &mut host, &mut store, 0, 64);
    play_ordered(&mut conditions, &mut host, &mut store, 64, 64);
    let expired = Outcome::Refused(Refusal::Expired);
    assert_eq!(
        log(&outcomes),
        [(1, Outcome::Applied { at: 30 }), (2, expired)]
    );
    assert_eq!(slots.taken(), 0);
}

#[test]
fn a_full_table_refuses_and_releases_the_slot() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(2);
    let edge = [Predicate::edges(0, 0, 1)];
    for id in 1..=3 {
        insert(&mut conditions, &slots, request(id, &edge), &mut store);
    }
    let full = Outcome::Refused(Refusal::PendingFull);
    assert_eq!(log(&outcomes), [(3, full)]);
    assert_eq!((conditions.len(), slots.taken()), (2, 2));
}

#[test]
fn an_install_maps_the_target_and_every_source_or_refuses() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let parts = [Predicate::edges(0, 0, 1), Predicate::edges(3, 0, 1)];
    for id in 1..=3 {
        let mut request = request(id, &parts);
        request.target.module_idx = id as usize;
        request.target.generation = INSTALLED - 1;
        insert(&mut conditions, &slots, request, &mut store);
    }
    // Module 2 (request 2's target) and module 3 (everyone's source)
    // removed for request 3's mapping only: target first.
    conditions.remap(INSTALLED, &mut store.outcomes, |_, module| match module {
        2 => None,
        3 => Some(7),
        module => Some(module + 10),
    });
    assert_eq!(log(&outcomes), [(2, Outcome::Refused(Refusal::TargetGone))]);
    assert_eq!(conditions.len(), 2);
    let entry = &conditions.entries[0];
    assert_eq!(entry.request.target.module_idx, 11);
    assert_eq!(entry.condition.parts()[0].module(), Some(10));
    assert_eq!(entry.condition.parts()[1].module(), Some(7));
    // Then module 10, a source of both, goes.
    conditions.remap(INSTALLED + 1, &mut store.outcomes, |_, module| {
        (module != 10).then_some(module)
    });
    let gone = Outcome::Refused(Refusal::SourceGone);
    assert_eq!(log(&outcomes), [(1, gone), (3, gone)]);
    assert_eq!(slots.taken(), 0);
}

#[test]
fn a_held_condition_waits_for_its_generation() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![gate(&[10], 64)]);
    let mut held = request(1, &[Predicate::edges(0, 0, 1)]);
    held.target.generation = INSTALLED + 1;
    insert(&mut conditions, &slots, held, &mut store);
    assert!(play_ordered(&mut conditions, &mut host, &mut store, 0, 64).is_empty());
    assert!(log(&outcomes).is_empty());
    assert!(!conditions.entries[0].armed);
}

#[test]
fn an_output_the_module_lacks_is_refused_when_armed() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let host = Host::new(vec![gate(&[10], 64)]);
    insert(
        &mut conditions,
        &slots,
        request(1, &[Predicate::edges(0, 4, 1)]),
        &mut store,
    );
    conditions.start(0, INSTALLED, &host, &mut store.outcomes);
    assert_eq!(log(&outcomes), [(1, Outcome::Refused(Refusal::Invalid))]);
}

#[test]
fn evaluating_neither_allocates_nor_frees() {
    let (mut store, _outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![gate(&[10, 20, 30], 64), gate(&[40], 64)]);
    let parts = [Predicate::edges(0, 0, 3), Predicate::edges(1, 0, 1)];
    insert(&mut conditions, &slots, request(1, &parts), &mut store);
    insert(&mut conditions, &slots, request(2, &parts[..1]), &mut store);
    let chained = [Predicate::Applied(RequestId(1)), control(1, Edge::Rising)];
    insert(&mut conditions, &slots, request(3, &chained), &mut store);
    host.segment(0);
    for module in 0..2 {
        host.compute(module, 64);
    }
    let ((), allocs, frees) = allocator_events(|| {
        conditions.start(0, INSTALLED, &host, &mut store.outcomes);
        conditions.read(INSTALLED, Some(TARGET), &host, &store.outcomes.chains);
        for at in [0, 30, 40] {
            host.now = at;
            apply_all(&mut conditions, &mut host, &mut store, at);
        }
        host.segment(64);
        host.controls[1] = 1.0;
        conditions.start(64, INSTALLED, &host, &mut store.outcomes);
        apply_all(&mut conditions, &mut host, &mut store, 64);
    });
    assert_eq!((allocs, frees), (0, 0));
    let applied = [(TARGET, 30), (TARGET, 40), (TARGET, 64)];
    assert_eq!(host.applied, applied);
}

/// A condition rides in every queue slot: keep a request narrow.
#[test]
fn a_condition_keeps_a_request_within_176_bytes() {
    assert_eq!(std::mem::size_of::<Predicate>(), 16);
    assert!(std::mem::size_of::<Request>() <= 176);
}

#[test]
fn the_sender_counts_the_conditions_in_flight() {
    let edge = [Predicate::edges(0, 0, 1)];
    let (sender, _consumer) = request_channel(4 * CONDITION_CAPACITY, Default::default());
    for id in 0..CONDITION_CAPACITY as u64 {
        assert!(!sender.watchers_full(false));
        sender.submit(request(id, &edge)).unwrap();
    }
    assert!(sender.watchers_full(false));
    // A request timed otherwise is not counted.
    sender
        .submit(Request::new(
            request(0, &edge).target,
            RequestValue::Value(RtValue::F32(0.0)),
        ))
        .unwrap();
    sender.watch_slots().release();
    assert!(!sender.watchers_full(false));
    assert_eq!(sender.watch_slots().taken(), CONDITION_CAPACITY - 1);
}

#[test]
fn a_full_queue_releases_the_slot_it_counted() {
    let (sender, _consumer) = request_channel(2, Default::default());
    let edge = [Predicate::edges(0, 0, 1)];
    let refused = (0..3).filter(|&id| sender.submit(request(id, &edge)).is_err());
    assert_eq!(refused.count(), 1);
    assert_eq!(sender.watch_slots().taken(), 2);
}

/// A rising crossing of `level` on control 0 of `module`.
fn control(module: u32, edge: Edge) -> Predicate {
    Predicate::Control {
        module,
        control: ControlIndex(0),
        edge,
        level: 0.5,
    }
}

#[test]
fn a_control_crossing_its_level_is_met_at_the_next_segment_start() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 256]; 2]);
    host.controls = vec![0.2, 0.8];
    let rising = request(1, &[control(0, Edge::Rising)]);
    let falling = request(2, &[control(1, Edge::Falling)]);
    insert(&mut conditions, &slots, rising, &mut store);
    insert(&mut conditions, &slots, falling, &mut store);
    play_ordered(&mut conditions, &mut host, &mut store, 0, 64);
    // Already past its level at arming: not a crossing.
    host.controls = vec![0.9, 0.9];
    play_ordered(&mut conditions, &mut host, &mut store, 64, 64);
    assert_eq!(log(&outcomes), [(1, Outcome::Applied { at: 64 })]);
    host.controls = vec![0.9, 0.1];
    play_ordered(&mut conditions, &mut host, &mut store, 128, 64);
    assert_eq!(log(&outcomes), [(2, Outcome::Applied { at: 128 })]);
    assert_eq!(slots.taken(), 0);
}

#[test]
fn a_request_waiting_on_another_applies_with_it() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![gate(&[30], 64)]);
    let first = request(1, &[Predicate::edges(0, 0, 1)]);
    let second = request(2, &[Predicate::Applied(RequestId(1))]);
    let third = request(3, &[Predicate::Applied(RequestId(2))]);
    insert(&mut conditions, &slots, first, &mut store);
    insert(&mut conditions, &slots, second, &mut store);
    insert(&mut conditions, &slots, third, &mut store);
    play_ordered(&mut conditions, &mut host, &mut store, 0, 64);
    let at_30 = Outcome::Applied { at: 30 };
    assert_eq!(log(&outcomes), [(1, at_30), (2, at_30), (3, at_30)]);
    assert_eq!(host.applied, [(TARGET, 30); 3]);
    assert_eq!(slots.taken(), 0);
}

#[test]
fn a_request_waiting_on_one_that_will_not_apply_is_refused() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    let waits = request(2, &[Predicate::Applied(RequestId(1))]);
    insert(&mut conditions, &slots, waits, &mut store);
    play_ordered(&mut conditions, &mut host, &mut store, 0, 64);
    let refused = Outcome::Refused(Refusal::Expired);
    store
        .outcomes
        .settle(request(1, &[Predicate::edges(0, 0, 1)]), refused);
    play_ordered(&mut conditions, &mut host, &mut store, 64, 64);
    let broken = Outcome::Refused(Refusal::ChainBroken);
    assert_eq!(log(&outcomes), [(1, refused), (2, broken)]);
}

#[test]
fn a_request_waiting_on_one_already_applied_applies_late() {
    let (mut store, outcomes) = store();
    let (mut conditions, slots) = table(4);
    let mut host = Host::new(vec![vec![0.0; 128]]);
    let applied = Outcome::Applied { at: 3 };
    store
        .outcomes
        .settle(request(1, &[Predicate::edges(0, 0, 1)]), applied);
    let waits = request(2, &[Predicate::Applied(RequestId(1))]);
    insert(&mut conditions, &slots, waits, &mut store);
    play_ordered(&mut conditions, &mut host, &mut store, 64, 64);
    let late = Outcome::AppliedLate { at: 64, due: 3 };
    assert_eq!(log(&outcomes), [(1, applied), (2, late)]);
}

mod reserve;
