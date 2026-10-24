//! Writes through the runtime's front door carry who made them: each is
//! acknowledged with the request it was submitted as and the value as
//! coerced, and ranks by its source and priority on the audio thread.

use std::collections::HashMap;

use super::declared::dial_rig;
use super::request_tiers::level;
use super::requests::{counted_block, outcomes};
use super::timed_status::front_door;
use super::*;
use crate::control_request::{Outcome, Source};
use crate::invention::declared::WriteOptions;
use crate::invention::orchestration::ControlAck;
use crate::{ControlValue, ControlWriteIntent};

const PERFORM: ControlWriteIntent = ControlWriteIntent::Perform;

/// Writes the dial's level through the front door as `options` say.
fn submit(
    rig: &Rig,
    value: ControlValue,
    intent: ControlWriteIntent,
    options: WriteOptions,
) -> ControlAck {
    front_door(rig)
        .submit_control("dial", "level", value, intent, options)
        .unwrap()
}

/// The outcomes reported since last taken, by request.
fn by_request(rig: &mut Rig) -> HashMap<u64, Outcome> {
    let outcomes = outcomes(rig).into_iter();
    outcomes.map(|(id, outcome)| (id.0, outcome)).collect()
}

fn id(ack: &ControlAck) -> u64 {
    ack.request.expect("submitted as a request").0
}

#[test]
fn a_users_write_outranks_a_later_scripts_at_one_sample() {
    let mut rig = dial_rig();
    let at = rig.graph.current_sample;
    let user = submit(
        &rig,
        0.75.into(),
        PERFORM,
        WriteOptions::performed(Source::User),
    );
    let script = submit(
        &rig,
        0.5.into(),
        PERFORM,
        WriteOptions::performed(Source::Script),
    );
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(level(&rig), 0.75);
    let outcomes = by_request(&mut rig);
    assert_eq!(outcomes[&id(&user)], Outcome::Applied { at });
    assert_eq!(outcomes[&id(&script)], Outcome::Overridden);
}

#[test]
fn an_agents_write_ranks_with_a_users_and_the_later_wins() {
    let mut rig = dial_rig();
    let user = submit(
        &rig,
        0.75.into(),
        PERFORM,
        WriteOptions::performed(Source::User),
    );
    let agent = submit(
        &rig,
        0.5.into(),
        PERFORM,
        WriteOptions::performed(Source::Agent),
    );
    counted_block(&mut rig);
    assert_eq!(level(&rig), 0.5);
    let outcomes = by_request(&mut rig);
    assert_eq!(outcomes[&id(&user)], Outcome::Superseded);
    assert!(matches!(outcomes[&id(&agent)], Outcome::Applied { .. }));
}

#[test]
fn priority_ranks_writes_from_one_source() {
    let mut rig = dial_rig();
    let high = WriteOptions {
        priority: 3,
        ..WriteOptions::performed(Source::User)
    };
    let first = submit(&rig, 0.75.into(), PERFORM, high);
    let second = submit(&rig, 0.5.into(), PERFORM, WriteOptions::default());
    counted_block(&mut rig);
    assert_eq!(level(&rig), 0.75);
    let outcomes = by_request(&mut rig);
    assert!(matches!(outcomes[&id(&first)], Outcome::Applied { .. }));
    assert_eq!(outcomes[&id(&second)], Outcome::Superseded);
}

#[test]
fn an_authored_write_is_acknowledged_with_its_request_and_coerced_value() {
    let mut rig = dial_rig();
    let author = ControlWriteIntent::Author;
    let ack = submit(&rig, "0.5".into(), author, WriteOptions::default());
    assert_eq!(ack.value, ControlValue::Number(0.5));
    counted_block(&mut rig);
    assert_eq!(level(&rig), 0.5);
    let outcomes = by_request(&mut rig);
    assert!(matches!(outcomes[&id(&ack)], Outcome::Applied { .. }));
}
