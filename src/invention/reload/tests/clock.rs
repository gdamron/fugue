//! Reloading a running clock: it keeps its beat position, unless its config
//! sets `reset_on_reload` (FUG-320).

use super::*;

/// A clock at 22500 bpm (128 samples a beat at 48 kHz) into the dac.
fn clock_doc(reset_on_reload: Option<bool>) -> String {
    let config = match reset_on_reload {
        Some(reset) => format!(r#"{{ "bpm": 22500, "reset_on_reload": {reset} }}"#),
        None => r#"{ "bpm": 22500 }"#.to_string(),
    };
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

/// Plays 10 blocks (5 beats), reloads the same document, plays one more
/// block, and returns the reload's report and the clock's position.
fn reload_after_five_beats(reset_on_reload: Option<bool>) -> (ReloadReport, f32) {
    let json = clock_doc(reset_on_reload);
    let mut running = start(&json);
    running.pump.render(10);
    assert_eq!(position(&running), 5.0);
    let report = running.reload(doc(&json)).expect("reload applies");
    running.pump.render(1);
    (report, position(&running))
}

#[test]
fn a_reload_keeps_the_beat_position_by_default() {
    for reset_on_reload in [None, Some(false)] {
        let (report, position) = reload_after_five_beats(reset_on_reload);
        assert!(report.controls_updated.is_empty(), "{report:?}");
        assert_eq!(position, 5.5, "{reset_on_reload:?}");
    }
}

#[test]
fn a_reload_resets_a_kept_clock_that_asks_for_it() {
    let (report, position) = reload_after_five_beats(Some(true));
    assert!(report.swapped.is_empty(), "kept, not rebuilt: {report:?}");
    assert_eq!(report.controls_updated, ["clock.reset"]);
    // Beat 0 at the block's first sample, so its last is at 63/128.
    assert_eq!(position, 63.0 / 128.0);
}

#[test]
fn a_reset_is_an_event_the_document_does_not_record() {
    let running = start(&clock_doc(None));
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

/// Plays 10 blocks (5 beats), reloads with the clock's `reset_on_reload`
/// changed from `before` to `after`, plays one more block, and returns the
/// reload's report and the clock's position.
fn reload_changing(before: Option<bool>, after: Option<bool>) -> (ReloadReport, f32) {
    let mut running = start(&clock_doc(before));
    running.pump.render(10);
    let report = running
        .reload(doc(&clock_doc(after)))
        .expect("reload applies");
    running.pump.render(1);
    (report, position(&running))
}

#[test]
fn changing_reset_on_reload_never_rebuilds_the_clock() {
    let cases = [
        (Some(true), Some(false), 5.5),
        (Some(true), None, 5.5),
        (None, Some(false), 5.5),
        (Some(false), Some(true), 63.0 / 128.0),
        (None, Some(true), 63.0 / 128.0),
    ];
    for (before, after, expected) in cases {
        let (report, position) = reload_changing(before, after);
        let context = format!("{before:?} -> {after:?}: {report:?}");
        assert!(report.swapped.is_empty(), "kept, not rebuilt: {context}");
        assert_eq!(position, expected, "{context}");
    }
    // The new setting is kept: a later reload of the same document reads it.
    let mut running = start(&clock_doc(Some(true)));
    running.pump.render(10);
    running.reload(doc(&clock_doc(Some(false)))).unwrap();
    let report = running.reload(doc(&clock_doc(Some(false)))).unwrap();
    assert!(report.controls_updated.is_empty(), "{report:?}");
    let document = running.document().unwrap();
    let clock = document.modules.iter().find(|m| m.id == "clock").unwrap();
    assert_eq!(clock.config["reset_on_reload"], false);
}
