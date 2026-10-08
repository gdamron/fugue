//! Live control writes refuse numbers that are not finite, on every intent and
//! every writer, leaving the module and the authored document untouched.

use super::*;
use crate::rpc::{RpcError, RpcErrorCode};
use crate::{ControlWrite, ControlWriteIntent};
use std::time::Instant;

const OSC_AND_DAC: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc", "type": "oscillator", "config": { "type": "sine", "frequency": 440.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

fn start(json: &str) -> RunningInvention {
    let invention = Invention::from_json(json).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap()
}

/// The oscillator's authored frequency, as the document's JSON holds it
/// (`null` for a number JSON cannot carry).
fn authored_frequency(running: &RunningInvention) -> serde_json::Value {
    let json = running.document().unwrap().to_json().unwrap();
    let document: serde_json::Value = serde_json::from_str(&json).unwrap();
    document["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|module| module["id"] == "osc")
        .map(|module| module["config"]["frequency"].clone())
        .unwrap()
}

/// Non-finite numbers, directly and as strings that coerce to one. `1e39` is
/// too large for an `f32` and becomes infinity.
fn non_finite_values() -> Vec<ControlValue> {
    vec![
        ControlValue::Number(f32::NAN),
        ControlValue::Number(f32::INFINITY),
        ControlValue::Number(f32::NEG_INFINITY),
        ControlValue::Number(1e39_f64 as f32),
        ControlValue::String("NaN".to_string()),
        ControlValue::String("inf".to_string()),
        ControlValue::String("-inf".to_string()),
        ControlValue::String("1e39".to_string()),
    ]
}

#[test]
fn set_control_refuses_non_finite_numbers_on_both_intents() {
    let running = start(OSC_AND_DAC);
    let sink = Arc::new(RecordingSink::default());
    running.set_event_sink(sink.clone());

    for intent in [ControlWriteIntent::Author, ControlWriteIntent::Perform] {
        for value in non_finite_values() {
            let error = running
                .snapshot()
                .set_control_with_intent("osc", "frequency", value.clone(), intent)
                .unwrap_err();
            let GraphCommandError::ControlError(message) = &error else {
                panic!("{value:?} ({intent:?}): expected a control error, got {error:?}");
            };
            assert!(
                message.starts_with("control 'osc.frequency' expects a finite number, got "),
                "{value:?} ({intent:?}): {message}"
            );
            assert_eq!(RpcError::from(error).code, RpcErrorCode::ControlError);
            assert_eq!(
                running.get_control("osc", "frequency").unwrap(),
                ControlValue::Number(440.0),
                "{value:?} ({intent:?}) left the module unchanged"
            );
            assert_eq!(
                authored_frequency(&running).as_f64(),
                Some(440.0),
                "{value:?} ({intent:?}) left the document unchanged"
            );
        }
    }
    assert!(
        sink.control_changes().is_empty(),
        "a refused write announces nothing"
    );

    // Finite extremes are still numbers, and land.
    for extreme in [f32::MAX, f32::MIN] {
        running
            .set_control("osc", "frequency", ControlValue::Number(extreme))
            .unwrap();
        let authored = authored_frequency(&running);
        assert_eq!(
            authored.as_f64().map(|number| number as f32),
            Some(extreme),
            "{authored}"
        );
    }
    running.stop();
}

#[test]
fn a_set_controls_batch_stops_at_a_non_finite_write() {
    let running = start(OSC_AND_DAC);

    let error = running
        .set_controls(&[
            ControlWrite::new("osc", "frequency", ControlValue::Number(550.0)),
            ControlWrite::new("osc", "frequency", ControlValue::Number(f32::NAN)),
            ControlWrite::new("osc", "frequency", ControlValue::Number(660.0)),
        ])
        .unwrap_err();
    assert!(
        error.to_string().contains("expects a finite number"),
        "{error}"
    );
    // The write before the refusal stands; the one after never runs.
    assert_eq!(
        running.get_control("osc", "frequency").unwrap(),
        ControlValue::Number(550.0)
    );
    assert_eq!(authored_frequency(&running).as_f64(), Some(550.0));
    running.stop();
}

#[test]
fn a_conducting_script_cannot_write_a_non_finite_number() {
    // Each attempt is caught in the script, which reports the outcomes as one
    // line through a control the test can read.
    let invention = r#"{
        "version": "1.0.0",
        "modules": [
            { "id": "osc", "type": "oscillator", "config": { "type": "sine", "frequency": 440.0 } },
            {
                "id": "code1",
                "type": "code",
                "config": {
                    "tick_hz": 20.0,
                    "script": "function tick() { if (globalThis.done) return; globalThis.done = true; const out = []; for (const v of [1e39, -1e39, 'NaN', 'Infinity', NaN, Infinity, -Infinity]) { try { graph.setControl('osc', 'frequency', v); out.push('accepted ' + v); } catch (e) { out.push(String(e)); } } graph.setControl('code1', 'last_error', out.join('|')); }"
                }
            },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    }"#;
    let running = start(invention);

    // Poll against a generous deadline: the script runs on its own tick
    // thread, which a loaded machine may start late.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut report = String::new();
    while Instant::now() < deadline {
        if let ControlValue::String(text) = running.get_control("code1", "last_error").unwrap() {
            if !text.is_empty() {
                report = text;
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    let frequency = running.get_control("osc", "frequency").unwrap();
    running.stop();

    assert!(!report.is_empty(), "script never reported its outcomes");
    let outcomes: Vec<&str> = report.split('|').collect();
    assert_eq!(outcomes.len(), 7, "{report}");
    for outcome in &outcomes {
        assert!(!outcome.starts_with("accepted"), "{report}");
    }
    // Numbers that reach the runtime (an overflowing literal, or a string
    // that coerces) get the edit path's refusal. JS NaN and the infinities
    // have no JSON form, so the script bridge refuses them before that.
    for outcome in &outcomes[..4] {
        assert!(
            outcome.contains("control 'osc.frequency' expects a finite number"),
            "{report}"
        );
    }
    assert_eq!(frequency, ControlValue::Number(440.0));
}
