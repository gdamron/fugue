//! Writes through the runtime's front door carry who made them: each is
//! acknowledged with the request it was submitted as and the value as
//! coerced, and ranks by its source and priority, live or offline.

use std::collections::HashMap;

use super::declared::dial_rig;
use super::request_tiers::level;
use super::requests::{counted_block, outcomes};
use super::timed_status::{front_door, queue_writing, BLOCKS};
use super::*;
use crate::control_request::{Outcome, Source};
use crate::invention::declared::{add_offline, WriteOptions};
use crate::invention::orchestration::ControlAck;
use crate::test_support::dial::DialFactory;
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

const SCHEDULED: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "sched", "type": "control_scheduler", "config": { "schedule": [
            { "at_step": 0, "module": "o", "control": "frequency", "value": 330.0 }
        ] } },
        { "id": "o", "type": "oscillator", "config": { "frequency": 440.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "o", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

const SCHEDULE: &str =
    r#"[{ "at_step": 0, "module": "o", "control": "frequency", "value": 220.0 }]"#;

#[test]
fn a_schedule_write_is_acknowledged_and_ranked_by_its_source() {
    let mut rig = Rig::new(SCHEDULED);
    let door = front_door(&rig);
    let schedule = |source| {
        let options = WriteOptions::performed(source);
        door.submit_control("sched", "schedule", SCHEDULE.into(), PERFORM, options)
            .unwrap()
    };
    let user = schedule(Source::User);
    let script = schedule(Source::Script);
    counted_block(&mut rig);
    let outcomes = by_request(&mut rig);
    assert!(matches!(outcomes[&id(&user)], Outcome::Applied { .. }));
    assert_eq!(outcomes[&id(&script)], Outcome::Overridden);
}

/// An offline render weighs writes at one sample as the audio thread does.
#[test]
fn an_offline_scripts_write_loses_to_a_users_at_one_sample() {
    let graph = Arc::new(Mutex::new(SignalGraph::new(
        IndexMap::new(),
        Vec::new(),
        Vec::new(),
        MasterObservers::default(),
    )));
    let mut registry = ModuleRegistry::default();
    registry.register(DialFactory);
    let json = serde_json::json!({});
    let dial = GraphChange::build(&registry, SAMPLE_RATE, "dial", "dial", &json).unwrap();
    let surface = dial.surface.clone().unwrap();
    let surfaces = Mutex::new(IndexMap::new());
    add_offline(
        &graph,
        &surfaces,
        "dial",
        dial.instance.unwrap(),
        Some(surface.clone()),
    )
    .unwrap();
    let write = |value: f32, source| {
        let options = WriteOptions::performed(source);
        surface.submit("level", value.into(), &options).unwrap();
        surface.get_control("level").unwrap()
    };
    assert_eq!(write(0.75, Source::User), 0.75.into());
    assert_eq!(write(0.5, Source::Script), 0.75.into(), "the user's holds");
    graph.lock().unwrap().current_sample += 1;
    assert_eq!(write(0.5, Source::Script), 0.5.into(), "a later sample");
}

/// An authored write kept over a timed edit that writes its control as it
/// lands expires as the write would: past its ttl, the edit's value plays.
#[test]
fn a_kept_authored_write_expires_with_its_ttl() {
    let mut rig = Rig::new(BASE);
    let edit = queue_writing(&rig, 440.0);
    rig.render(1);
    let options = WriteOptions {
        ttl: Some(64),
        ..WriteOptions::default()
    };
    let author = ControlWriteIntent::Author;
    let door = front_door(&rig);
    door.submit_control("osc2", "frequency", 660.0.into(), author, options)
        .unwrap();
    assert_eq!(
        rig.live.timed_status(edit).unwrap().kept,
        ["osc2.frequency"]
    );
    rig.render(BLOCKS);
    let playing = door.get_control("osc2", "frequency").unwrap();
    assert_eq!(playing, ControlValue::Number(440.0));
    let expired = Outcome::Refused(crate::control_request::Refusal::Expired);
    assert!(by_request(&mut rig)
        .values()
        .any(|outcome| *outcome == expired));
}
