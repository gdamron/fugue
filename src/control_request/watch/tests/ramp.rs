//! Ramps in beats starting from watches on a scripted timeline: which
//! are refused, where one starts, what replaces one, and the room each
//! takes as it arrives.

use super::ramps::RampHost;
use super::*;
use crate::control_request::hold::Rank;
use crate::control_request::ramps::RAMP_CAPACITY;
use crate::control_request::request::Source;

/// A ramp to `to` over `beats`, `after` beats after its clock's position,
/// on control 0 of module 1.
fn ramp(id: u64, after: f32, to: f32, beats: f32) -> Request {
    let mut request = request(id, 1, after);
    request.value = RequestValue::Value(RtValue::F32(to));
    if let When::Beat(time) = &mut request.when {
        time.ramp = Some(beats);
    }
    request.source = Source::Automation;
    request
}

/// A segment start at the host's sample, as the graph runs one: every
/// watch due, then every ramp, then the watches again until none applies.
/// Returns the segment's length, at most 64.
fn segment(watches: &mut Watches, host: &mut RampHost, store: &mut PendingStore) -> u64 {
    let now = host.now;
    loop {
        if watches.pass(now, INSTALLED, host, &mut store.outcomes).0 {
            continue;
        }
        let arrives = watches.ramps.step(INSTALLED, host, &mut store.outcomes);
        let (applied, wait) = watches.pass(now, INSTALLED, host, &mut store.outcomes);
        if !applied {
            return [wait, arrives].into_iter().flatten().fold(64, u64::min);
        }
    }
}

/// Plays segments until sample `until`.
fn play_until(watches: &mut Watches, host: &mut RampHost, store: &mut PendingStore, until: u64) {
    while host.now < until {
        let length = segment(watches, host, store).min(until - host.now);
        host.play(length);
    }
}

#[test]
fn a_ramp_is_refused_unless_it_writes_a_finite_number_once_in_beats() {
    for beats in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            Watches::check(&ramp(1, 1.0, 1.0, beats)),
            Err(Refusal::Invalid)
        );
    }
    assert_eq!(
        Watches::check(&ramp(1, 1.0, f32::NAN, 1.0)),
        Err(Refusal::Invalid)
    );
    let mut whole = ramp(1, 1.0, 1.0, 1.0);
    whole.value = RequestValue::Value(RtValue::I32(1));
    assert_eq!(Watches::check(&whole), Err(Refusal::Unsupported));
    let mut event = ramp(1, 1.0, 1.0, 1.0);
    event.event = true;
    assert_eq!(Watches::check(&event), Err(Refusal::Unsupported));
    let mut repeating = ramp(1, 1.0, 1.0, 1.0);
    repeating.when = When::Beat(BeatTime {
        clock: 0,
        spec: BeatSpec::Grid {
            every: 1.0,
            offset: 0.0,
            repeat: true,
        },
        ramp: Some(1.0),
    });
    assert_eq!(Watches::check(&repeating), Err(Refusal::Unsupported));
    assert_eq!(Watches::check(&ramp(1, 1.0, 1.0, 0.25)), Ok(()));
}

#[test]
fn a_ramp_starts_on_its_beat_from_where_its_control_stands() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    watches.insert(ramp(1, 1.0, 1.0, 2.0), 0, &mut store.outcomes);
    let mut host = RampHost::new(0.25);
    // Sample n is at position (n + 1) / 128: beat 1 falls on sample 127,
    // beat 3 on sample 383.
    play_until(&mut watches, &mut host, &mut store, 1024);
    assert_eq!(log(&outcomes), [(1, Outcome::Applied { at: 127 })]);
    assert!(watches.is_empty(), "it ends silently once it arrives");
    let (first, last) = (host.writes[0], *host.writes.last().unwrap());
    assert_eq!(first, (127, 0.25), "it takes the control where it stands");
    assert_eq!(last, (383, 1.0));
    // Every write stands as the request did at its beat.
    let start = Standing::of(&ramp(1, 1.0, 1.0, 2.0), 127);
    assert!(host.standings.iter().all(|standing| *standing == start));
    assert_eq!(start.unwrap().rank, Rank::new(Source::Automation, 0));
}

#[test]
fn a_ramp_held_at_its_beat_never_starts() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    watches.insert(ramp(1, 1.0, 1.0, 2.0), 0, &mut store.outcomes);
    let mut host = RampHost::new(0.0);
    host.held = true;
    play_until(&mut watches, &mut host, &mut store, 1024);
    assert_eq!(log(&outcomes), [(1, Outcome::Overridden)]);
    assert!(watches.is_empty() && host.writes.is_empty());
}

#[test]
fn another_write_to_its_control_ends_a_ramp() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    watches.insert(ramp(1, 1.0, 1.0, 2.0), 0, &mut store.outcomes);
    // A one-shot to the same control, at beat 2.
    watches.insert(request(2, 1, 2.0), 0, &mut store.outcomes);
    let mut host = RampHost::new(0.0);
    play_until(&mut watches, &mut host, &mut store, 1024);
    assert_eq!(
        log(&outcomes),
        [
            (1, Outcome::Applied { at: 127 }),
            (2, Outcome::Applied { at: 255 })
        ]
    );
    assert_eq!(*host.writes.last().unwrap(), (255, 1.0));
    assert!(watches.is_empty());
}

#[test]
fn a_ramp_on_a_control_holding_no_number_is_refused() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(4);
    let mut elsewhere = ramp(1, 1.0, 1.0, 2.0);
    elsewhere.target.module_idx = 2;
    watches.insert(elsewhere, 0, &mut store.outcomes);
    let mut host = RampHost::new(f32::INFINITY);
    watches.insert(ramp(2, 1.0, 1.0, 2.0), 0, &mut store.outcomes);
    play_until(&mut watches, &mut host, &mut store, 256);
    let unsupported = Outcome::Refused(Refusal::Unsupported);
    assert_eq!(log(&outcomes), [(1, unsupported), (2, unsupported)]);
    assert!(watches.is_empty() && host.writes.is_empty());
}

#[test]
fn a_ramp_takes_its_room_at_intake_and_ends_silently_when_its_target_goes() {
    let (mut store, outcomes) = store();
    let mut watches = Watches::new(RAMP_CAPACITY + 1);
    let mut old = ramp(1, 1.0, 1.0, 2.0);
    old.target.generation = INSTALLED - 1;
    watches.insert(old, 0, &mut store.outcomes);
    watches.remap(INSTALLED, &mut store.outcomes, |_, module_idx| {
        Some(module_idx)
    });
    let mut host = RampHost::new(0.0);
    play_until(&mut watches, &mut host, &mut store, 200);
    assert_eq!(log(&outcomes), [(1, Outcome::Applied { at: 127 })]);
    // Running, it takes a ramp's room: the rest go to ramps waiting.
    for id in 2..=RAMP_CAPACITY as u64 {
        watches.insert(ramp(id, 8.0, 1.0, 1.0), 0, &mut store.outcomes);
    }
    assert!(log(&outcomes).is_empty());
    let full = RAMP_CAPACITY as u64 + 1;
    watches.insert(ramp(full, 8.0, 1.0, 1.0), 0, &mut store.outcomes);
    // A write without a ramp still has a watch.
    watches.insert(request(full + 1, 1, 8.0), 0, &mut store.outcomes);
    let pending_full = Outcome::Refused(Refusal::PendingFull);
    assert_eq!(log(&outcomes), [(full, pending_full)]);
    watches.remap(INSTALLED + 1, &mut store.outcomes, |_, _| None);
    assert!(watches.is_empty());
    let gone = log(&outcomes);
    assert_eq!(gone.len(), RAMP_CAPACITY, "every watch, but no ramp");
    assert!(gone.iter().all(|&(id, _)| id != 1));
}

#[test]
fn starting_and_running_a_ramp_neither_allocates_nor_frees() {
    let (mut store, _outcomes) = store();
    let mut watches = Watches::new(4);
    watches.insert(ramp(1, 1.0, 1.0, 2.0), 0, &mut store.outcomes);
    watches.insert(ramp(2, 1.5, 1.0, 0.5), 0, &mut store.outcomes);
    let mut host = RampHost::new(0.0);
    let ((), allocs, frees) =
        allocator_events(|| play_until(&mut watches, &mut host, &mut store, 1024));
    assert_eq!((allocs, frees), (0, 0));
    assert!(watches.is_empty());
}
