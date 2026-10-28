//! A reserved request (a note off) timed in beats finds room among the
//! watches when others, waiting far off, fill them: the drain takes it
//! into the reserve rather than refuse it. And a timed note off passes the
//! front doors' bound on writes waiting, by as many as the reserves hold.

use super::request_beats::{beat_rig, blocks_for, level_change, render_counted, submit_level};
use super::requests::outcomes;
use super::sample_instrument;
use super::timed_status::front_door;
use super::Rig;
use crate::control_request::{
    BeatSpec, BeatTime, ControlIndex, Outcome, Refusal, Request, RequestValue, RtValue, Source,
    When, WAITING_RESERVE,
};
use crate::invention::declared::WriteOptions;
use crate::invention::publish::publisher::BEAT_WATCH_CAPACITY;
use crate::invention::publish::timed_write::{SAMPLE_TIMED_CAPACITY, TIMED_RESERVE};
use crate::invention::publish::WriteTime;
use crate::test_support::dial::LEVEL;
use crate::{ControlValue, ControlWriteIntent};

#[test]
fn full_watches_leave_a_reserved_beat_request_its_room() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    let far = BeatSpec::After(1.0e6);
    for _ in 0..BEAT_WATCH_CAPACITY {
        submit_level(&rig, 0.5, far);
    }
    let reserved = |level: f32| {
        let publisher = rig.live.publisher().lock().unwrap();
        let target = publisher.control_target("dial", LEVEL).unwrap();
        let clock = publisher.control_target("clock", ControlIndex(0)).unwrap();
        let mut request = Request::new(target, RequestValue::Value(RtValue::F32(level)));
        request.when = When::Beat(BeatTime {
            clock: clock.module_idx as u32,
            spec: BeatSpec::After(1.0),
            ramp: None,
        });
        request.reserved = true;
        rig.live.requests.submit(request).unwrap()
    };
    for _ in 1..WAITING_RESERVE {
        reserved(0.75);
    }
    let ends = reserved(0.75);
    let beyond = reserved(0.75);
    let refused = submit_level(&rig, 0.5, far);
    let out = render_counted(&mut rig, blocks_for(120.0, 1.0) + 1);
    assert!(
        level_change(&out, 0.75).is_some(),
        "the reserved ones apply"
    );
    let settled = outcomes(&mut rig);
    let full = Outcome::Refused(Refusal::PendingFull);
    assert!(settled.contains(&(beyond, full)), "{settled:?}");
    assert!(settled.contains(&(refused, full)), "{settled:?}");
    let applied =
        |(id, outcome): &(_, Outcome)| *id == ends && matches!(outcome, Outcome::Applied { .. });
    assert!(settled.iter().any(applied), "{settled:?}");
}

/// Writes `value` to the instrument's `key` on sample `at`, through the
/// front door.
fn write_at(rig: &Rig, key: &str, value: f32, at: u64) -> Result<(), String> {
    let options = WriteOptions {
        time: Some(WriteTime::AtSample(at)),
        ..WriteOptions::performed(Source::Agent)
    };
    let perform = ControlWriteIntent::Perform;
    let value = ControlValue::Number(value);
    let written = front_door(rig).submit_control("inst", key, value, perform, options);
    written.map(drop).map_err(|error| error.to_string())
}

#[test]
fn a_timed_note_off_passes_the_bound_on_writes_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let rig = sample_instrument::rig(dir.path());
    let far = rig.graph.current_sample + 10_000_000;
    for n in 0..SAMPLE_TIMED_CAPACITY as u64 {
        write_at(&rig, "release", 0.5, far + n).unwrap();
    }
    let error = write_at(&rig, "release", 0.5, far).unwrap_err();
    assert!(error.contains("already wait on a sample"), "{error}");
    for n in 0..TIMED_RESERVE as u64 {
        write_at(&rig, "note_off", 60.0, far + n).unwrap();
    }
    let error = write_at(&rig, "note_off", 60.0, far).unwrap_err();
    assert!(error.contains("already wait on a sample"), "{error}");
}
