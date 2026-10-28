//! A reserved request (a note off) timed in beats finds room among the
//! watches when others, waiting far off, fill them: the drain takes it
//! into the reserve rather than refuse it.

use super::request_beats::{beat_rig, blocks_for, level_change, render_counted, submit_level};
use super::requests::outcomes;
use crate::control_request::{
    BeatSpec, BeatTime, ControlIndex, Outcome, Refusal, Request, RequestValue, RtValue, When,
    WAITING_RESERVE,
};
use crate::invention::publish::publisher::BEAT_WATCH_CAPACITY;
use crate::test_support::dial::LEVEL;

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
