//! The front doors leave a reserved write (a note off) room past their
//! bounds on writes waiting: on a sample, in all, and among the requests
//! timed by a condition, through either door that times one. So other
//! writes waiting, which may never apply, never shut out the end of a
//! note, and the reserves stay bounded.

use super::requests::outcomes;
use super::sample_instrument;
use super::timed_status::front_door;
use super::*;
use crate::control_request::{
    BeatSpec, Outcome, Refusal, RequestId, Source, CONDITION_CAPACITY, WAITING_RESERVE,
};
use crate::invention::declared::WriteOptions;
use crate::invention::publish::publisher::BEAT_WATCH_CAPACITY;
use crate::invention::publish::timed_write::{SAMPLE_TIMED_CAPACITY, SAMPLE_TIMED_RESERVE};
use crate::invention::publish::watch::Part;
use crate::invention::publish::WriteTime;
use crate::{ControlValue, ControlWriteIntent};
use serde_json::json;

/// Writes `value` to the instrument's `key` at `time`, through the front
/// door; the write's request id.
fn write(rig: &Rig, key: &str, value: f32, time: WriteTime<'_>) -> Result<RequestId, String> {
    let options = WriteOptions {
        time: Some(time),
        ..WriteOptions::performed(Source::Agent)
    };
    let perform = ControlWriteIntent::Perform;
    let value = ControlValue::Number(value);
    let written = front_door(rig).submit_control("inst", key, value, perform, options);
    let ack = written.map_err(|error| error.to_string())?;
    Ok(ack.request.expect("submitted as a request"))
}

/// Writes `value` to the instrument's `key` once request `after` applies,
/// through `submit_when`.
fn write_when(rig: &Rig, key: &str, value: f32, after: RequestId) -> Result<RequestId, String> {
    let value = ControlValue::Number(value);
    let parts = [Part::Applied(after)];
    let written = rig.live.submit_when(("inst", key), &value, &parts, None);
    written.map_err(|error| error.to_string())
}

#[test]
fn a_timed_note_off_passes_the_bound_on_writes_waiting_on_a_sample() {
    let dir = tempfile::tempdir().unwrap();
    let rig = sample_instrument::rig(dir.path());
    let far = rig.graph.current_sample + 10_000_000;
    for n in 0..SAMPLE_TIMED_CAPACITY as u64 {
        write(&rig, "release", 0.5, WriteTime::AtSample(far + n)).unwrap();
    }
    let error = write(&rig, "release", 0.5, WriteTime::AtSample(far)).unwrap_err();
    assert!(error.contains("already wait on a sample"), "{error}");
    for n in 0..SAMPLE_TIMED_RESERVE as u64 {
        write(&rig, "note_off", 60.0, WriteTime::AtSample(far + n)).unwrap();
    }
    let error = write(&rig, "note_off", 60.0, WriteTime::AtSample(far)).unwrap_err();
    assert!(error.contains("already wait on a sample"), "{error}");
}

/// Note offs on a sample take their own reserve, so one timed on a beat
/// still passes the bound on timed writes once every other kind is full.
#[test]
fn a_note_off_on_a_beat_passes_the_bound_on_every_timed_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = sample_instrument::rig(dir.path());
    let clock = rig.build("clock", "clock", json!({ "bpm": 120.0 }));
    rig.live
        .edit(|change: &mut GraphChange| {
            change.upsert("clock", clock);
            Ok(())
        })
        .unwrap();
    rig.render(1);
    let far = rig.graph.current_sample + 10_000_000;
    let on_sample = |n: u64| WriteTime::AtSample(far + n);
    let beat = WriteTime::Beat {
        clock: Some("clock"),
        spec: BeatSpec::After(1.0e6),
    };
    let first = write(&rig, "release", 0.5, on_sample(0)).unwrap();
    let after = WriteTime::After {
        request: first,
        offset: 0,
    };
    // A block now and then takes them up, so the queue never fills.
    for n in 1..SAMPLE_TIMED_CAPACITY as u64 {
        write(&rig, "release", 0.5, on_sample(n)).unwrap();
    }
    rig.render(1);
    for n in 0..BEAT_WATCH_CAPACITY {
        write(&rig, "release", n as f32 / 100.0, beat).unwrap();
    }
    // Each waits on a request of its own, so none coalesces.
    for n in 0..CONDITION_CAPACITY {
        write_when(&rig, "release", 0.5, RequestId(u64::MAX - n as u64)).unwrap();
    }
    rig.render(1);
    let error = write(&rig, "release", 0.5, beat).unwrap_err();
    assert!(error.contains("timed writes already wait"), "{error}");
    for n in 0..SAMPLE_TIMED_RESERVE as u64 {
        write(&rig, "note_off", 60.0, on_sample(n)).unwrap();
    }
    write(&rig, "note_off", 60.0, beat).unwrap();
    write(&rig, "note_off", 61.0, after).unwrap();
}

/// Both doors that time a write by a condition count the requests waiting
/// on one, and leave a note off the reserve: it then waits in the table,
/// never refused for want of room.
#[test]
fn a_note_off_timed_by_a_condition_finds_room_through_either_door() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = sample_instrument::rig(dir.path());
    let far = rig.graph.current_sample + 10_000_000;
    let first = write(&rig, "release", 0.5, WriteTime::AtSample(far)).unwrap();
    let after = WriteTime::After {
        request: first,
        offset: 0,
    };
    // Each waits on a request of its own, so none coalesces.
    let never = |n: usize| RequestId(u64::MAX - n as u64);
    for n in 0..CONDITION_CAPACITY {
        write_when(&rig, "release", 0.5, never(n)).unwrap();
    }
    assert!(write_when(&rig, "release", 0.5, never(999)).is_err());
    assert!(write(&rig, "release", 0.5, after).is_err());
    for n in 0..WAITING_RESERVE / 2 {
        write_when(&rig, "note_off", 60.0, never(1000 + n)).unwrap();
        write(&rig, "note_off", (61 + n) as f32, after).unwrap();
    }
    assert!(write_when(&rig, "note_off", 60.0, never(2000)).is_err());
    assert!(write(&rig, "note_off", 60.0, after).is_err());
    rig.render(4);
    let full = Outcome::Refused(Refusal::PendingFull);
    let settled = outcomes(&mut rig);
    assert!(
        !settled.iter().any(|(_, outcome)| *outcome == full),
        "{settled:?}"
    );
}
