//! Control writes timed through the front door on a live graph: one on a
//! beat lands where a request timed on that beat does, one after another
//! request lands that many samples after it applies (waiting still, or
//! applied already), and every block stays allocation- and free-free. A
//! time that cannot come, or a write that cannot wait for it, is refused
//! before anything is queued.

use super::request_beats::{beat_rig, blocks_for, level_change, render_counted, submit_level};
use super::requests::outcomes;
use super::timed_status::front_door;
use super::*;
use crate::control_request::{BeatSpec, Outcome, RequestId, Source};
use crate::invention::declared::WriteOptions;
use crate::invention::orchestration::ControlAck;
use crate::invention::publish::WriteTime;
use crate::test_support::host_dial::{HostDialFactory, HOST_DIAL};
use crate::{ControlValue, ControlWriteIntent};

/// Performs the dial's `level` at `time` through the front door.
fn level_at(rig: &Rig, level: f32, time: WriteTime<'_>) -> Result<ControlAck, String> {
    write_at(rig, ("dial", "level"), level, time)
}

fn write_at(
    rig: &Rig,
    (module_id, key): (&str, &str),
    value: f32,
    time: WriteTime<'_>,
) -> Result<ControlAck, String> {
    let options = WriteOptions {
        time: Some(time),
        ..WriteOptions::performed(Source::Agent)
    };
    let perform = ControlWriteIntent::Perform;
    front_door(rig)
        .submit_control(
            module_id,
            key,
            ControlValue::Number(value),
            perform,
            options,
        )
        .map_err(|error| error.to_string())
}

fn id(ack: &ControlAck) -> RequestId {
    ack.request.expect("submitted as a request")
}

#[test]
fn a_write_after_another_request_lands_that_many_samples_after_it() {
    let mut rig = beat_rig(97.0);
    let start = rig.graph.current_sample;
    let first = level_at(&rig, 0.5, WriteTime::AtSample(start + 100)).unwrap();
    let after = |ack: &ControlAck, offset| WriteTime::After {
        request: id(ack),
        offset,
    };
    let second = level_at(&rig, 0.75, after(&first, 211)).unwrap();
    let out = render_counted(&mut rig, 8);
    assert_eq!(level_change(&out, 0.5), Some(100));
    assert_eq!(level_change(&out, 0.75), Some(311), "211 samples after");

    // After one applied already, from the sample it applied on.
    let now = rig.graph.current_sample;
    let third = level_at(&rig, 0.125, after(&second, 64 * 9)).unwrap();
    let out = render_counted(&mut rig, 8);
    let heard = level_change(&out, 0.125).map(|i| now + i as u64);
    assert_eq!(heard, Some(start + 311 + 64 * 9));
    let landed = outcomes(&mut rig);
    let applied = Outcome::Applied {
        at: start + 311 + 64 * 9,
    };
    assert!(landed.contains(&(id(&third), applied)), "{landed:?}");
}

#[test]
fn a_write_on_a_beat_lands_where_a_request_on_that_beat_does() {
    let spec = BeatSpec::After(1.5);
    let mut rig = beat_rig(97.0);
    let mut twin = beat_rig(97.0);
    rig.render(3);
    twin.render(3);
    let beat = WriteTime::Beat { clock: None, spec };
    level_at(&rig, 0.5, beat).unwrap();
    submit_level(&twin, 0.5, spec);
    let blocks = blocks_for(97.0, 1.5);
    let heard = level_change(&render_counted(&mut rig, blocks), 0.5);
    assert!(heard.is_some());
    assert_eq!(heard, level_change(&render_counted(&mut twin, blocks), 0.5));
    // On a clock named, which must be one.
    let on_clock = WriteTime::Beat {
        clock: Some("clock"),
        spec,
    };
    assert!(level_at(&rig, 0.25, on_clock).is_ok());
    let on_dial = WriteTime::Beat {
        clock: Some("dial"),
        spec,
    };
    assert!(level_at(&rig, 0.25, on_dial).is_err());
}

#[test]
fn a_write_after_a_request_that_never_applies_or_none_is_refused() {
    let mut rig = beat_rig(97.0);
    let unknown = WriteTime::After {
        request: RequestId(987_654),
        offset: 0,
    };
    let error = level_at(&rig, 0.5, unknown).unwrap_err();
    assert!(error.contains("no request 987654"), "{error}");

    // Replaced at its sample by a later write, the first never applies.
    let at = rig.graph.current_sample + 10;
    let lost = level_at(&rig, 0.5, WriteTime::AtSample(at)).unwrap();
    level_at(&rig, 0.75, WriteTime::AtSample(at)).unwrap();
    render_counted(&mut rig, 2);
    let after_lost = WriteTime::After {
        request: id(&lost),
        offset: 0,
    };
    let error = level_at(&rig, 0.25, after_lost).unwrap_err();
    assert!(error.contains("never applied"), "{error}");
}

#[test]
fn a_write_that_cannot_wait_for_its_time_is_refused() {
    let mut rig = beat_rig(97.0);
    rig.registry.register(HostDialFactory);
    let host = rig.build("fx", HOST_DIAL, serde_json::json!({}));
    // A script's status, too, which its own surface writes at once.
    let script = rig.build("script", "code", serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("fx", host);
            change.upsert("script", script);
            Ok(())
        })
        .unwrap();
    let soon = WriteTime::AfterSamples(64);
    let error = write_at(&rig, ("fx", "rate"), 2.0, soon).unwrap_err();
    assert!(error.contains("cannot be timed"), "{error}");
    let options = WriteOptions {
        time: Some(soon),
        ..WriteOptions::performed(Source::Agent)
    };
    let status = ControlValue::String("timed".into());
    let perform = ControlWriteIntent::Perform;
    let refused = front_door(&rig).submit_control("script", "last_error", status, perform, options);
    let error = refused.unwrap_err().to_string();
    assert!(error.contains("cannot be timed"), "{error}");
    assert!(rig.live.pending_requests().is_empty(), "nothing queued");
}
