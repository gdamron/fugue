//! A running clock's reset: an event the document does not record
//! (FUG-320).

use super::*;

/// A clock at 22500 bpm (128 samples a beat at 48 kHz) into the dac.
fn clock_doc() -> String {
    let config = r#"{ "bpm": 22500 }"#;
    format!(
        r#"{{
        "version": "1.0.0",
        "modules": [
            {{ "id": "clock", "type": "clock", "config": {config} }},
            {{ "id": "dac", "type": "dac", "config": {{ "soft_clip": false }} }}
        ],
        "connections": [
            {{ "from": "clock", "from_port": "beat", "to": "dac", "to_port": "audio" }}
        ]
    }}"#
    )
}

fn position(running: &Settled) -> f32 {
    match running.running.get_control("clock", "position").unwrap() {
        ControlValue::Number(position) => position,
        other => panic!("position is {other:?}"),
    }
}

#[test]
fn a_reset_is_an_event_the_document_does_not_record() {
    let running = start(&clock_doc());
    running.pump.render(10);
    running
        .set_control("clock", "reset", ControlValue::Bool(true))
        .unwrap();
    let audio = running.pump.render(1);
    // The reset lands at the block's start: its first sample is beat 0, a
    // gate a quarter beat long.
    assert!(audio[..32].iter().all(|&sample| sample > 0.0), "{audio:?}");
    assert!(audio[32..].iter().all(|&sample| sample == 0.0), "{audio:?}");
    assert_eq!(position(&running), 63.0 / 128.0);
    let document = running.document().unwrap();
    let clock = document.modules.iter().find(|m| m.id == "clock").unwrap();
    assert!(clock.config.get("reset").is_none(), "{:?}", clock.config);
}
