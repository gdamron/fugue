//! A condition's parts on their own: crossing a level each way, joining
//! parts, the bounds on how many, and what nothing can meet.

use super::*;
use crate::control_request::Request;

#[test]
fn a_crossing_goes_strictly_past_the_level_from_at_most_or_at_least_it() {
    let rising = |last, now| Edge::Rising.crosses(last, now, GATE_LEVEL);
    assert!(rising(0.0, 1.0) && rising(0.5, 0.6));
    assert!(!rising(0.6, 1.0) && !rising(0.0, 0.5) && !rising(1.0, 0.0));
    let falling = |last, now| Edge::Falling.crosses(last, now, 0.25);
    assert!(falling(1.0, 0.0) && falling(0.25, 0.2));
    assert!(!falling(0.2, 0.0) && !falling(1.0, 0.25) && !falling(0.0, 1.0));
    // A NaN sample crosses nothing, to it or from it.
    assert!(!rising(0.0, f32::NAN) && !rising(f32::NAN, 1.0));
}

#[test]
fn edges_count_rising_gates_on_an_output() {
    let edges = Predicate::edges(3, 1, 16);
    let expected = Predicate::Signal {
        module: 3,
        port: 1,
        edge: Edge::Rising,
        level: GATE_LEVEL,
        count: 16,
    };
    assert_eq!(edges, expected);
    assert_eq!(edges.module(), Some(3));
    assert_eq!(Predicate::Applied(RequestId(4)).module(), None);
}

#[test]
fn all_is_met_when_its_last_part_is() {
    let met = |parts: &[Option<u64>]| Join::All.met(parts.iter().copied());
    assert_eq!(met(&[Some(30)]), Some(30));
    assert_eq!(met(&[Some(30), Some(12), Some(45), Some(0)]), Some(45));
    assert_eq!(met(&[Some(30), None]), None);
}

#[test]
fn a_condition_joins_one_to_four_parts() {
    let part = |count| Predicate::edges(0, 0, count);
    assert_eq!(Condition::all(&[]), None);
    assert_eq!(Condition::all(&[part(1); 5]), None);
    let four = [part(1), part(2), part(3), part(4)];
    for len in 1..=MAX_PARTS {
        let condition = Condition::all(&four[..len]).unwrap();
        assert_eq!(condition.parts(), &four[..len]);
        assert_eq!(condition.join(), Join::All);
    }
    let mut condition = Condition::all(&four[..2]).unwrap();
    for part in condition.parts_mut() {
        *part.module_mut().unwrap() = 7;
    }
    assert!(condition
        .parts()
        .iter()
        .all(|part| part.module() == Some(7)));
}

#[test]
fn a_part_nothing_can_meet_is_refused() {
    let never = Predicate::edges(0, 0, 0);
    let nowhere = Predicate::Control {
        module: 0,
        control: ControlIndex(1),
        edge: Edge::Falling,
        level: f32::INFINITY,
    };
    let fine = Predicate::edges(0, 0, 1);
    assert_eq!(Condition::all(&[fine]).unwrap().check(), Ok(()));
    for bad in [never, nowhere] {
        let condition = Condition::all(&[fine, bad]).unwrap();
        assert_eq!(condition.check(), Err(Refusal::Invalid));
    }
}

/// A condition rides in every queue slot: keep a request narrow.
#[test]
fn a_condition_keeps_a_request_within_176_bytes() {
    assert_eq!(std::mem::size_of::<Predicate>(), 16);
    assert!(std::mem::size_of::<Request>() <= 176);
}
