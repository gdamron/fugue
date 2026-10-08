//! Requests timed on the sample transport: `AtSample(n)` and
//! `AfterSamples(n)` apply exactly at their sample whatever the block
//! lengths, a request already past applies late and says so, and one whose
//! ttl runs out is refused, unapplied. Every block here is allocation- and
//! free-free.

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
fn a_request_held_for_its_publication_past_its_ttl_is_refused() {
    let (mut rig, port) = oscillator_rig();
    rig.hold_a_retirement();
    let osc3 = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc3));
    // Due now, at the block that holds it, and good for 10 samples only.
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

/// The twin of `deferred_installs_bound_the_folded_remaps_by_back_pressure`
/// with requests that expire while held: they keep their room until the
/// install, so producers still meet `QueueFull` and the folded remaps stay
/// bounded by the store and queue capacity.
#[test]
fn requests_expiring_while_installs_are_deferred_keep_the_back_pressure() {
    use crate::invention::publish::publisher::{PENDING_REQUEST_CAPACITY, REQUEST_QUEUE_CAPACITY};
    let (mut rig, port) = oscillator_rig();
    rig.hold_a_retirement();
    let fm = edge("osc1", "audio", "osc2", "frequency_mod");
    let (per_round, rounds) = (100, 12);
    let mut written_rounds = 0;
    let mut queue_full = 0;
    for round in 0..rounds {
        rig.publish_unreclaimed(|change| {
            if round % 2 == 0 {
                change.connect(fm.clone()).unwrap();
            } else {
                change.disconnect(fm.clone());
            }
        });
        let mut written = false;
        for n in 0..per_round {
            let when = When::AfterSamples(64 + n);
            match try_submit_with_ttl(&rig, "osc1", port, 1.0, when, 0) {
                Some(_) => written = true,
                None => queue_full += 1,
            }
        }
        written_rounds += usize::from(written);
        assert_eq!(counted_block(&mut rig), (0, 0), "round {round}");
    }
    let capacity = PENDING_REQUEST_CAPACITY + REQUEST_QUEUE_CAPACITY;
    assert_eq!(queue_full, rounds * per_round as usize - capacity);
    assert_eq!(outcomes(&mut rig), [], "nothing was settled while held");
    let absorbed = {
        let publisher = rig.live.publisher().lock().unwrap();
        publisher.pending_absorbed().unwrap()
    };
    assert_eq!(absorbed.len(), written_rounds);
    assert_eq!(written_rounds, capacity.div_ceil(per_round as usize));

    // Once installed, every one of them is refused for its ttl.
    rig.live.reclaim();
    assert_eq!(counted_block(&mut rig), (0, 0), "installing");
    let settled = outcomes(&mut rig);
    assert_eq!(settled.len(), capacity);
    assert!(settled
        .iter()
        .all(|(_, outcome)| *outcome == Outcome::Refused(Refusal::Expired)));
}

/// A held request with a ttl cannot know whether it applies until its
/// publication installs, so it replaces nothing: whichever it would have
/// replaced still applies when it expires.
#[test]
fn a_held_request_that_expires_never_erases_the_one_before_it() {
    let (mut rig, port) = oscillator_rig();
    rig.hold_a_retirement();
    let osc3 = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc3));
    let start = rig.graph.transport.rendered();
    let at = When::AtSample(start + 10);
    // Same control and sample: the second can never apply in time, the
    // third could have, but its publication installs too late.
    let first = submit(&rig, "osc3", port, 0.5, at);
    let never = submit_with_ttl(&rig, "osc3", port, 0.25, at, 5);
    let too_late = submit_with_ttl(&rig, "osc3", port, 0.75, at, 20);

    assert_eq!(counted_block(&mut rig), (0, 0), "holding");
    assert_eq!(outcomes(&mut rig), [], "nothing superseded while held");
    rig.live.reclaim();
    let installed = rig.graph.current_sample;
    assert_eq!(counted_block(&mut rig), (0, 0), "installing");
    assert_eq!(frequency(&mut rig, "osc3", port), 0.5);
    assert_eq!(
        outcomes(&mut rig),
        [
            // Refused as the install maps them, before anything applies.
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

/// Requests held for a publication and expired by the time it installs
/// leave at the install, however far off their sample: otherwise a store
/// full of them would back-pressure every producer until those samples.
#[test]
fn an_install_refuses_expired_held_requests_at_once_and_restores_intake() {
    use crate::invention::publish::publisher::{PENDING_REQUEST_CAPACITY, REQUEST_QUEUE_CAPACITY};
    let (mut rig, port) = oscillator_rig();
    rig.hold_a_retirement();
    let osc3 = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc3));
    // Fill the store, a queue at a time.
    for batch in 0..(PENDING_REQUEST_CAPACITY / REQUEST_QUEUE_CAPACITY) as u64 {
        for n in 0..REQUEST_QUEUE_CAPACITY as u64 {
            let far = When::AfterSamples(1_000_000 + batch * 1_000 + n);
            submit_with_ttl(&rig, "osc3", port, 0.5, far, 0);
        }
        assert_eq!(counted_block(&mut rig), (0, 0), "holding");
    }
    assert_eq!(
        rig.graph.requests.as_ref().unwrap().pending.len(),
        PENDING_REQUEST_CAPACITY
    );
    assert_eq!(outcomes(&mut rig), []);

    rig.live.reclaim();
    assert_eq!(counted_block(&mut rig), (0, 0), "installing");
    let settled = outcomes(&mut rig);
    assert_eq!(settled.len(), PENDING_REQUEST_CAPACITY);
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
