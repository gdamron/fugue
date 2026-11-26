//! The write rule as a daemon applies it: each batch, and what becomes of
//! it.

use crate::invention::manual_backend::Settled;
use crate::rpc::{ControlWriteReason, RpcErrorCode};
use crate::{ControlValue, ControlWrite, Invention, InventionBuilder};

const DOCUMENT: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
        { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
        { "id": "mix", "type": "mixer", "config": { "levels": [0.5, 0.5], "master": 0.8 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "mix", "to_port": "audio.0" },
        { "from": "mix", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
        { "from": "mix", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
    ]
}"#;

/// What became of a batch: each write's value and whether it was
/// clamped, or the refusal's index, reason and message.
#[derive(Debug, PartialEq)]
enum Outcome {
    Made(Vec<(ControlValue, bool)>),
    Refused(usize, ControlWriteReason, String),
}

fn write(module_id: &str, key: &str, value: ControlValue) -> ControlWrite {
    ControlWrite::new(module_id, key, value)
}

fn text(value: &str) -> ControlValue {
    ControlValue::String(value.into())
}

fn refusal(error: crate::rpc::RpcError) -> Outcome {
    assert_eq!(error.code, RpcErrorCode::InvalidControlWrite, "{error:?}");
    let write = error.write.clone().expect("names the write");
    assert!(error.message.contains(&write.message), "{error:?}");
    Outcome::Refused(write.index, write.reason, write.message)
}

fn on_daemon(running: &Settled, writes: &[ControlWrite]) -> Outcome {
    let acks = running.write_controls(writes);
    if let Some(error) = acks.error {
        assert!(acks.controls.is_empty());
        return refusal(error);
    }
    Outcome::Made(
        (writes.iter().zip(acks.controls))
            .map(|(write, ack)| {
                let value = running.get_control(&write.module_id, &write.key).unwrap();
                assert_eq!(ack.requested.as_ref(), Some(&value), "{ack:?}");
                (value, ack.clamped)
            })
            .collect(),
    )
}

/// Each batch, and what becomes of it.
fn cases() -> Vec<(Vec<ControlWrite>, Outcome)> {
    use ControlWriteReason::*;
    let num = ControlValue::Number;
    let made = |values: &[(ControlValue, bool)]| Outcome::Made(values.to_vec());
    let refused = |index, reason, message: &str| Outcome::Refused(index, reason, message.into());
    vec![
        // Out of range: the nearer limit, said to be clamped.
        (
            vec![write("mix", "level.0", num(7.0))],
            made(&[(num(2.0), true)]),
        ),
        (
            vec![write("mix", "master", num(-1.0))],
            made(&[(num(0.0), true)]),
        ),
        (
            vec![write("osc", "frequency", num(-5.0))],
            made(&[(num(0.0), true)]),
        ),
        // Within the range a control holds, wider than it lists.
        (
            vec![write("osc", "frequency", num(5.0))],
            made(&[(num(5.0), false)]),
        ),
        (
            vec![write("clock", "bpm", num(1000.0))],
            made(&[(num(1000.0), false)]),
        ),
        // Loose forms.
        (
            vec![write("mix", "level.0", text("0.7"))],
            made(&[(num(0.7), false)]),
        ),
        (
            vec![write("mix", "level.0", text(" 3 "))],
            made(&[(num(2.0), true)]),
        ),
        (
            vec![write("osc", "waveform", text("Sine"))],
            made(&[(text("sine"), false)]),
        ),
        (
            vec![write("osc", "waveform", text("SQUARE"))],
            made(&[(text("square"), false)]),
        ),
        // Refused.
        (
            vec![write("gone", "level.0", num(0.5))],
            refused(0, UnknownModule, "no module 'gone' exists"),
        ),
        (
            vec![write("mix", "volume", num(0.5))],
            refused(
                0,
                UnknownControl,
                "module 'mix' has no control 'volume' (available: ",
            ),
        ),
        (
            vec![write("clock", "position", num(4.0))],
            refused(
                0,
                InvalidControlValue,
                "module 'clock' refused the value for control 'position': ",
            ),
        ),
        (
            vec![write("mix", "level.0", text("loud"))],
            refused(
                0,
                InvalidControlValue,
                "control 'mix.level.0' expects a number, got \"loud\"",
            ),
        ),
        (
            vec![write("mix", "level.0", ControlValue::Bool(true))],
            refused(
                0,
                InvalidControlValue,
                "control 'mix.level.0' expects a number, got true",
            ),
        ),
        (
            vec![write("mix", "level.0", text("NaN"))],
            refused(
                0,
                InvalidControlValue,
                "control 'mix.level.0' expects a finite number, got NaN",
            ),
        ),
        (
            vec![write("mix", "level.0", text("inf"))],
            refused(
                0,
                InvalidControlValue,
                "control 'mix.level.0' expects a finite number, got inf",
            ),
        ),
        (
            vec![write("osc", "waveform", text("kazoo"))],
            refused(
                0,
                InvalidControlValue,
                "module 'osc' refused the value for control 'waveform': ",
            ),
        ),
        // A batch: all or none.
        (
            vec![
                write("mix", "level.0", num(0.25)),
                write("osc", "frequency", num(330.0)),
                write("mix", "volume", num(0.5)),
            ],
            refused(
                2,
                UnknownControl,
                "module 'mix' has no control 'volume' (available: ",
            ),
        ),
        (
            vec![
                write("mix", "level.0", num(9.0)),
                write("mix", "level.1", num(-2.0)),
                write("mix", "master", num(1.5)),
            ],
            made(&[(num(2.0), true), (num(0.0), true), (num(1.5), false)]),
        ),
    ]
}

/// A refusal's message as far as `expected` gives it: the start of one
/// that lists what is available, or the whole.
fn matches(outcome: &Outcome, expected: &Outcome) -> bool {
    match (outcome, expected) {
        (Outcome::Refused(index, reason, message), Outcome::Refused(i, r, m)) => {
            (index, reason) == (i, r)
                && match m.ends_with(": ") || m.ends_with("(available: ") {
                    true => message.starts_with(m.as_str()),
                    false => message == m,
                }
        }
        (made, expected) => made == expected,
    }
}

#[test]
fn a_daemon_accepts_clamps_and_refuses_writes_by_the_rule() {
    let invention = Invention::from_json(DOCUMENT).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = Settled::start(runtime);
    for (writes, expected) in cases() {
        let daemon = on_daemon(&running, &writes);
        assert!(matches(&daemon, &expected), "{writes:?}: {daemon:?}");
    }
    // The refused batch wrote nothing: the frequency is the 5 Hz written
    // before it, not its 330 Hz.
    let frequency = running.get_control("osc", "frequency").unwrap();
    assert_eq!(frequency, ControlValue::Number(5.0));
}

#[test]
fn a_mixed_batch_with_a_timed_write_that_cannot_land_queues_nothing() {
    use crate::rpc::{ControlTiming, ControlWhen};
    let invention = Invention::from_json(DOCUMENT).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = Settled::start(runtime);
    let timed = |when| ControlWrite {
        timing: ControlTiming {
            when: Some(when),
            ttl_ms: None,
        },
        ..write("osc", "frequency", ControlValue::Number(440.0))
    };
    let refusals = [
        // Timed on a module that is no clock.
        ControlWhen::AfterBeats {
            beats: 1.0,
            clock: Some("mix".into()),
        },
        // After a request never made.
        ControlWhen::AfterRequest {
            request: "999999".into(),
            offset_samples: 0,
        },
    ];
    for when in refusals {
        let writes = [
            write("mix", "level.0", ControlValue::Number(0.1)),
            timed(when.clone()),
        ];
        let acks = running.write_controls(&writes);
        let error = acks.error.expect("refused");
        let refused = error.write.expect("names the write");
        assert_eq!(
            (refused.index, refused.reason),
            (1, ControlWriteReason::InvalidTiming),
            "{when:?}"
        );
        assert!(acks.controls.is_empty());
        assert_eq!(
            running.get_control("mix", "level.0").unwrap(),
            ControlValue::Number(0.5)
        );
        assert!(running
            .pending_requests(None, crate::rpc::PendingDetail::Full)
            .items
            .is_empty());
    }
    // A ttl alone is refused as sent too.
    let mut bounded = write("osc", "frequency", ControlValue::Number(1.0));
    bounded.timing.ttl_ms = Some(10);
    let acks =
        running.write_controls(&[write("mix", "level.0", ControlValue::Number(0.1)), bounded]);
    let refused = acks.error.expect("refused").write.expect("names the write");
    assert_eq!(
        (refused.index, refused.reason),
        (1, ControlWriteReason::InvalidTiming)
    );
    assert_eq!(
        running.get_control("mix", "level.0").unwrap(),
        ControlValue::Number(0.5)
    );
}

#[test]
fn a_batch_writes_a_control_its_count_write_brings_into_play() {
    let document = r#"{"version":"1.0.0","modules":[
        {"id":"tune","type":"melody","config":{"degree_count":3}},
        {"id":"dac","type":"dac"}],"connections":[]}"#;
    let invention = Invention::from_json(document).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = Settled::start(runtime);
    let acks = running.write_controls(&[
        write("tune", "degree_count", ControlValue::Number(7.0)),
        write("tune", "degree.6", text("2")),
    ]);
    assert!(acks.error.is_none(), "{:?}", acks.error);
    let read = running.get_control("tune", "degree.6").unwrap();
    assert_eq!(read, ControlValue::Number(2.0));
}
