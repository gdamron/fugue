//! Requests timed on the sample transport: `AtSample(n)` and
//! `AfterSamples(n)` apply exactly at their sample whatever the block
//! lengths, a request already past applies late and says so, and one whose
//! ttl runs out is refused, unapplied. Every block here is allocation- and
//! free-free.

use std::time::Instant;

use super::requests::{counted_block, frequency, level_rig, oscillator_rig, outcomes, submit};
use super::*;
use crate::control_request::{
    ControlIndex, Outcome, Refusal, Request, RequestId, RequestValue, RtValue, When,
};

/// Submits `value` for `control` of `module_id` as a front door would, with
/// a ttl, unless the queue is full.
fn try_submit_with_ttl(
    rig: &Rig,
    module_id: &str,
    control: u16,
    value: f32,
    when: When,
    ttl: u64,
) -> Option<RequestId> {
    let mut publisher = rig.live.publisher().lock().unwrap();
    let target = publisher
        .control_target(module_id, ControlIndex(control))
        .unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(value)));
    request.when = when;
    request.ttl = Some(ttl);
    let id = rig.live.requests.submit(request).ok()?;
    publisher.note_written();
    Some(id)
}

/// [`try_submit_with_ttl`], which must find room.
fn submit_with_ttl(
    rig: &Rig,
    module_id: &str,
    control: u16,
    value: f32,
    when: When,
    ttl: u64,
) -> RequestId {
    try_submit_with_ttl(rig, module_id, control, value, when, ttl).unwrap()
}

/// Renders `frames` frames in blocks of the given lengths, cycling, and
/// returns the left channel. Every block must be allocation- and free-free.
fn render_in_blocks(rig: &mut Rig, frames: usize, lengths: &[usize]) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    let mut left = [0.0f32; 64];
    let mut right = [0.0f32; 64];
    for &n in lengths.iter().cycle() {
        let n = n.min(frames - out.len());
        if n == 0 {
            break;
        }
        let ((), allocs, frees) =
            allocator_events(|| rig.graph.process_block(&mut left[..n], &mut right[..n]));
        assert_eq!((allocs, frees), (0, 0), "block of {n}");
        out.extend_from_slice(&left[..n]);
    }
    out
}

#[test]
fn the_audio_thread_publishes_its_sample_count_after_every_block() {
    let mut rig = level_rig();
    let start = rig.graph.current_sample;
    assert_eq!(rig.graph.transport.rendered(), start);
    render_in_blocks(&mut rig, 100, &[17, 64, 5]);
    assert_eq!(rig.graph.current_sample, start + 100);
    assert_eq!(rig.graph.transport.rendered(), start + 100);
}

#[test]
fn sample_timed_requests_apply_exactly_at_their_sample_whatever_the_blocks() {
    for blocks in [&[64][..], &[17, 64, 5, 33], &[1, 2, 3]] {
        for offset in [0, 1, 2, 16, 17, 63, 64, 65, 127, 128, 129, 191] {
            for relative in [false, true] {
                let mut rig = level_rig();
                let start = rig.graph.transport.rendered();
                let when = if relative {
                    When::AfterSamples(offset)
                } else {
                    When::AtSample(start + offset)
                };
                let id = submit(&rig, "level", 0, 1.0, when);
                let out = render_in_blocks(&mut rig, 256, blocks);
                let first = out.iter().position(|v| *v == 1.0);
                let label = format!("{blocks:?}, offset {offset}, relative {relative}");
                assert_eq!(first, Some(offset as usize), "{label}");
                assert!(out[offset as usize..].iter().all(|v| *v == 1.0), "{label}");
                let applied = Outcome::Applied { at: start + offset };
                assert_eq!(outcomes(&mut rig), [(id, applied)], "{label}");
            }
        }
    }
}

#[test]
fn after_samples_counts_from_the_count_published_at_submission() {
    let mut rig = level_rig();
    // A submitter that read the count three blocks ago.
    let start = rig.graph.current_sample;
    rig.render(3);
    rig.graph.transport.publish(start);
    let on_time = submit(&rig, "level", 0, 0.5, When::AfterSamples(64 * 3 + 10));
    let late = submit(&rig, "level", 0, 0.25, When::AfterSamples(64));
    let now = rig.graph.current_sample;

    let out = rig.render(1);
    assert!(out[..10].iter().all(|v| *v == 0.25), "{out:?}");
    assert!(out[10..].iter().all(|v| *v == 0.5), "{out:?}");
    assert_eq!(
        outcomes(&mut rig),
        [
            (
                late,
                Outcome::AppliedLate {
                    at: now,
                    due: start + 64
                }
            ),
            (on_time, Outcome::Applied { at: now + 10 }),
        ]
    );
}

#[test]
fn a_request_that_could_only_apply_after_its_ttl_is_refused_unapplied() {
    let (mut rig, port) = oscillator_rig();
    let start = rig.graph.transport.rendered();
    let kept = submit(&rig, "osc1", port, 0.5, When::AtSample(start + 10));
    // Same control and sample, but expiring at `start + 5`: it must not
    // supersede the one waiting.
    let expired = submit_with_ttl(&rig, "osc1", port, 0.25, When::AtSample(start + 10), 5);
    // Due at `start + 10`, expiring there: still in time.
    let just = submit_with_ttl(&rig, "osc2", port, 0.75, When::AfterSamples(10), 10);

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(frequency(&mut rig, "osc1", port), 0.5);
    assert_eq!(frequency(&mut rig, "osc2", port), 0.75);
    assert_eq!(
        outcomes(&mut rig),
        [
            (expired, Outcome::Refused(Refusal::Expired)),
            (kept, Outcome::Applied { at: start + 10 }),
            (just, Outcome::Applied { at: start + 10 }),
        ]
    );
}

#[test]
fn a_late_request_applies_late_within_its_ttl_and_is_refused_past_it() {
    let (mut rig, port) = oscillator_rig();
    let start = rig.graph.transport.rendered();
    // Already past, but its ttl reaches the next block's start.
    let within = submit_with_ttl(&rig, "osc1", port, 0.5, When::AtSample(start - 10), 0);
    // A submitter that read the count a block before the audio thread
    // took its request: a ttl of 32 ran out before `start`.
    rig.graph.transport.publish(start - 64);
    let past = submit_with_ttl(&rig, "osc2", port, 0.25, When::Now, 32);

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(frequency(&mut rig, "osc1", port), 0.5);
    assert_ne!(frequency(&mut rig, "osc2", port), 0.25);
    assert_eq!(
        outcomes(&mut rig),
        [
            // Refused as it is taken, before anything due applies.
            (past, Outcome::Refused(Refusal::Expired)),
            (
                within,
                Outcome::AppliedLate {
                    at: start,
                    due: start - 10
                }
            ),
        ]
    );
}

#[test]
fn a_request_queued_behind_its_edit_past_its_ttl_is_refused() {
    let (mut rig, port) = oscillator_rig();
    rig.hold_a_retirement();
    let osc3 = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc3));
    // Due now, good for 10 samples only, but it waits a block behind the
    // edit that adds osc3.
    let id = submit_with_ttl(&rig, "osc3", port, 0.5, When::Now, 10);
    let held = submit(&rig, "osc3", port, 0.25, When::AfterSamples(64 + 3));
    let start = rig.graph.current_sample;

    assert_eq!(counted_block(&mut rig), (0, 0), "holding");
    rig.live.reclaim();
    assert_eq!(counted_block(&mut rig), (0, 0), "installing");
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "osc3"]);
    assert_eq!(frequency(&mut rig, "osc3", port), 0.25);
    assert_eq!(
        outcomes(&mut rig),
        [
            (id, Outcome::Refused(Refusal::Expired)),
            (held, Outcome::Applied { at: start + 64 + 3 }),
        ]
    );
}

/// A request queued behind its edit is placed once the edit installs, and
/// one expired by then is refused as it is taken, replacing nothing:
/// whichever it would have replaced still applies.
#[test]
fn a_request_expired_behind_its_edit_never_erases_the_one_before_it() {
    let (mut rig, port) = oscillator_rig();
    rig.hold_a_retirement();
    let osc3 = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc3));
    let start = rig.graph.transport.rendered();
    let at = When::AtSample(start + 10);
    // Same control and sample: the second can never apply in time, the
    // third could have, but its edit installs too late.
    let first = submit(&rig, "osc3", port, 0.5, at);
    let never = submit_with_ttl(&rig, "osc3", port, 0.25, at, 5);
    let too_late = submit_with_ttl(&rig, "osc3", port, 0.75, at, 20);

    assert_eq!(counted_block(&mut rig), (0, 0), "holding");
    assert_eq!(outcomes(&mut rig), [], "nothing taken while the edit waits");
    rig.live.reclaim();
    let installed = rig.graph.current_sample;
    assert_eq!(counted_block(&mut rig), (0, 0), "installing");
    assert_eq!(frequency(&mut rig, "osc3", port), 0.5);
    assert_eq!(
        outcomes(&mut rig),
        [
            // Refused as they are taken, before anything applies.
            (never, Outcome::Refused(Refusal::Expired)),
            (too_late, Outcome::Refused(Refusal::Expired)),
            (
                first,
                Outcome::AppliedLate {
                    at: installed,
                    due: start + 10
                }
            ),
        ]
    );
}

/// Requests queued behind a deferred edit wait in the queue, not the
/// store, so a backlog of them back-pressures producers at the door;
/// once the edit installs, those already expired leave at once, however
/// far off their sample, and intake is open again.
#[test]
fn requests_behind_a_deferred_edit_back_pressure_at_the_queue() {
    use crate::invention::publish::publisher::REQUEST_QUEUE_CAPACITY;
    let (mut rig, port) = oscillator_rig();
    rig.hold_a_retirement();
    let osc3 = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc3));
    let mut queued = 0;
    while try_submit_with_ttl(
        &rig,
        "osc3",
        port,
        0.5,
        When::AfterSamples(1_000_000 + queued),
        0,
    )
    .is_some()
    {
        queued += 1;
    }
    // The edit holds one of the slots control requests may fill.
    assert_eq!(queued, REQUEST_QUEUE_CAPACITY as u64 - 1);
    assert_eq!(counted_block(&mut rig), (0, 0), "waiting to install");
    assert_eq!(rig.graph.requests.as_ref().unwrap().pending.len(), 0);
    assert_eq!(outcomes(&mut rig), []);

    rig.live.reclaim();
    assert_eq!(counted_block(&mut rig), (0, 0), "installing");
    let settled = outcomes(&mut rig);
    assert_eq!(settled.len() as u64, queued);
    assert!(settled
        .iter()
        .all(|(_, outcome)| *outcome == Outcome::Refused(Refusal::Expired)));
    assert_eq!(rig.graph.requests.as_ref().unwrap().pending.len(), 0);

    // Intake is open again at once.
    let start = rig.graph.current_sample;
    let id = submit(&rig, "osc3", port, 0.25, When::Now);
    assert_eq!(counted_block(&mut rig), (0, 0), "after the install");
    assert_eq!(frequency(&mut rig, "osc3", port), 0.25);
    assert_eq!(outcomes(&mut rig), [(id, Outcome::Applied { at: start })]);
}

/// A refused request needs no room in the store, so behind a full store it
/// is settled at once rather than blocking the queue: a replacement behind
/// it still lands on its sample.
#[test]
fn an_expired_request_never_blocks_a_replacement_behind_a_full_store() {
    use crate::invention::publish::publisher::{PENDING_REQUEST_CAPACITY, REQUEST_QUEUE_CAPACITY};
    let (mut rig, port) = oscillator_rig();
    let far = rig.graph.current_sample + 1_000_000;
    let fill = REQUEST_QUEUE_CAPACITY as u64;
    for n in 0..fill {
        submit(&rig, "osc1", port, 0.1, When::AtSample(far + n));
    }
    assert_eq!(counted_block(&mut rig), (0, 0));
    for n in fill..PENDING_REQUEST_CAPACITY as u64 - 1 {
        submit(&rig, "osc1", port, 0.1, When::AtSample(far + n));
    }
    let at = rig.graph.current_sample + 64 + 10;
    let first = submit(&rig, "osc2", port, 0.5, When::AtSample(at));
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert!(rig.graph.requests.as_ref().unwrap().pending.is_full());

    // Expiring before its sample, then a replacement for the same sample.
    let expired = submit_with_ttl(&rig, "osc2", port, 0.75, When::AtSample(at), 5);
    let replacement = submit(&rig, "osc2", port, 0.25, When::AtSample(at));
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(frequency(&mut rig, "osc2", port), 0.25);
    assert_eq!(
        outcomes(&mut rig),
        [
            (expired, Outcome::Refused(Refusal::Expired)),
            (first, Outcome::Superseded),
            (replacement, Outcome::Applied { at }),
        ]
    );
}

#[test]
fn a_wall_clock_request_applies_at_the_sample_heard_then() {
    let mut rig = level_rig();
    let start = rig.graph.transport.rendered();
    // The next sample is heard now, so 20 ms on is 960 samples on.
    let now = Instant::now();
    rig.graph.transport.start_clock(now, SAMPLE_RATE);
    rig.graph.transport.anchor(start, now);
    let id = submit(
        &rig,
        "level",
        0,
        1.0,
        When::AtTime(now + Duration::from_millis(20)),
    );

    let out = rig.render(16);
    assert_eq!(out.iter().position(|v| *v == 1.0), Some(960));
    assert_eq!(
        outcomes(&mut rig),
        [(id, Outcome::Applied { at: start + 960 })]
    );
}

#[test]
fn without_a_wall_clock_a_wall_clock_request_is_refused() {
    let mut rig = level_rig();
    let id = submit(&rig, "level", 0, 1.0, When::AtTime(Instant::now()));
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(
        outcomes(&mut rig),
        [(id, Outcome::Refused(Refusal::NoClock))]
    );
}

#[test]
fn an_unplaced_wall_clock_request_behind_its_edit_waits_queued() {
    let (mut rig, port) = oscillator_rig();
    rig.hold_a_retirement();
    let osc3 = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc3));
    let unplaced = submit(&rig, "osc3", port, 0.5, When::AtTime(Instant::now()));
    let behind = submit(&rig, "osc3", port, 0.25, When::Now);

    assert_eq!(counted_block(&mut rig), (0, 0), "holding");
    assert_eq!(rig.graph.requests.as_ref().unwrap().pending.len(), 0);
    assert_eq!(outcomes(&mut rig), []);
    rig.live.reclaim();
    let start = rig.graph.current_sample;
    assert_eq!(counted_block(&mut rig), (0, 0), "installing");
    assert_eq!(frequency(&mut rig, "osc3", port), 0.25);
    assert_eq!(
        outcomes(&mut rig),
        [
            (unplaced, Outcome::Refused(Refusal::NoClock)),
            (behind, Outcome::Applied { at: start }),
        ]
    );
}

/// Submitted before the first callback anchors the clock, a wall-clock
/// time is placed by the audio thread once it has: a stream that starts
/// late, or with more latency, moves it.
#[test]
fn a_wall_clock_request_submitted_before_the_first_anchor_waits_for_it() {
    let mut rig = level_rig();
    let start = rig.graph.transport.rendered();
    let now = Instant::now();
    rig.graph.transport.start_clock(now, SAMPLE_RATE);
    let id = submit(
        &rig,
        "level",
        0,
        1.0,
        When::AtTime(now + Duration::from_millis(20)),
    );
    // The first callback: its first sample is heard 5 ms after `now`.
    rig.graph
        .transport
        .anchor(start, now + Duration::from_millis(5));

    // Placed on the audio thread, allocation- and free-free.
    let out = render_in_blocks(&mut rig, 1024, &[64]);
    assert_eq!(out.iter().position(|v| *v == 1.0), Some(720));
    assert_eq!(
        outcomes(&mut rig),
        [(id, Outcome::Applied { at: start + 720 })]
    );
}
