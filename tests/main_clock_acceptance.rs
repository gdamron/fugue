//! Acceptance tests for a piece's optional main clock, written from the spec
//! alone.
//!
//! Spec summary: an invention may name a clock module's id in an optional
//! `main_clock` field (left out of the JSON when unset). The status
//! summary lists clocks with the main clock first, then the rest in document
//! order (`Invention::summary_clocks`); with no main clock, document order. An
//! unknown or non-clock id is dropped on load with a warning that names it.
//! Changing only `main_clock` in a reload rebuilds nothing: the clock keeps
//! playing, and the retained document carries the new value.
//!
//! Run with `cargo test --test main_clock_acceptance`.

use std::sync::{Arc, Mutex};

use fugue::{
    AudioBackend, ControlValue, Invention, InventionBuilder, InventionRuntime, RunningInvention,
};
use serde_json::{json, Value};

const RATE: u32 = 48_000;

type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

struct Capture(Arc<Mutex<Option<Render>>>);

impl AudioBackend for Capture {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn start(&mut self, render: Render) -> Result<(), Box<dyn std::error::Error>> {
        *self.0.lock().unwrap() = Some(render);
        Ok(())
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().take();
    }
}

/// Clocks `first`, `second` and `third`, with an oscillator between them.
fn document(main_clock: Option<&str>) -> Value {
    let mut doc = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "first", "type": "clock", "config": { "bpm": 225.0 } },
            { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
            { "id": "second", "type": "clock", "config": { "bpm": 90.0 } },
            { "id": "third", "type": "clock", "config": { "bpm": 90.0 } },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    });
    if let Some(id) = main_clock {
        doc["main_clock"] = json!(id);
    }
    doc
}

fn parse(main_clock: Option<&str>) -> Invention {
    serde_json::from_value(document(main_clock)).unwrap()
}

fn build(main_clock: Option<&str>) -> InventionRuntime {
    InventionBuilder::new(RATE)
        .build(parse(main_clock))
        .unwrap()
        .0
}

fn start(main_clock: Option<&str>) -> (RunningInvention, Arc<Mutex<Option<Render>>>) {
    let slot = Arc::new(Mutex::new(None));
    let running = build(main_clock)
        .start_with_backend(Capture(slot.clone()))
        .unwrap();
    (running, slot)
}

fn blocks(slot: &Mutex<Option<Render>>, count: usize) {
    let mut slot = slot.lock().unwrap();
    let render = slot.as_mut().expect("backend started");
    for _ in 0..count {
        render(&mut [0.0; 64], &mut [0.0; 64]);
    }
}

fn position(running: &RunningInvention, clock: &str) -> f32 {
    match running.get_control(clock, "position") {
        Ok(ControlValue::Number(n)) => n,
        other => panic!("{other:?}"),
    }
}

/// "The main clock first when it designates one, then the others in the order
/// the piece lists them"; "With no main clock, the first clock in the piece
/// leads."
#[test]
fn clocks_are_listed_with_the_main_clock_first_then_in_document_order() {
    let ids = |main: Option<&str>| {
        let invention = parse(main);
        invention
            .summary_clocks()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(None),
        ["first", "second", "third"],
        "no oscillator or dac"
    );
    assert_eq!(ids(Some("third")), ["third", "first", "second"]);
    assert_eq!(ids(Some("second")), ["second", "first", "third"]);
    assert_eq!(ids(Some("first")), ["first", "second", "third"]);
}

/// "An unknown or non-clock id is ignored with a warning": dropped from the
/// retained document, named in the load warnings, and the order falls back to
/// the document's.
#[test]
fn an_unknown_or_non_clock_main_clock_is_dropped_with_a_warning_naming_it() {
    for bad in ["ghost", "osc"] {
        let runtime = build(Some(bad));
        let warnings = runtime.load_warnings();
        assert!(
            warnings.iter().any(|w| w.contains(bad)),
            "a warning names {bad}: {warnings:?}"
        );
        let slot = Arc::new(Mutex::new(None));
        let running = runtime.start_with_backend(Capture(slot)).unwrap();
        let document = running.document().expect("a retained document");
        assert_eq!(document.main_clock, None, "{bad} is dropped");
        assert_eq!(document.summary_clocks(), ["first", "second", "third"]);
        running.stop();
    }
    // A fine one warns of nothing.
    let warnings = build(Some("third")).load_warnings();
    assert!(
        warnings.iter().all(|w| !w.contains("third")),
        "{warnings:?}"
    );
}

/// "It is saved with the piece"; "with no field present, documents parse and
/// save exactly as before."
#[test]
fn a_main_clock_is_saved_with_the_piece_and_left_out_when_unset() {
    let saved: Value = serde_json::from_str(&parse(Some("third")).to_json().unwrap()).unwrap();
    assert_eq!(saved["main_clock"], json!("third"));
    let again = Invention::from_json(&saved.to_string()).unwrap();
    assert_eq!(again.main_clock.as_deref(), Some("third"));

    let plain = parse(None);
    assert_eq!(plain.main_clock, None);
    let saved: Value = serde_json::from_str(&plain.to_json().unwrap()).unwrap();
    assert!(saved.get("main_clock").is_none(), "{saved}");

    let (running, _slot) = start(Some("second"));
    let document = running.document().unwrap();
    assert_eq!(document.main_clock.as_deref(), Some("second"));
    running.stop();
}

/// "Changing it rebuilds nothing": a reload that only changes `main_clock`
/// leaves every module in place, the clock keeps counting, and the retained
/// document carries the new value (and then none again).
#[test]
fn changing_only_the_main_clock_in_a_reload_rebuilds_nothing() {
    let (mut running, slot) = start(None);
    blocks(&slot, 500);
    let before = position(&running, "first");
    assert!(before > 1.0, "{before}");

    for (to, expected) in [(Some("third"), Some("third")), (None, None)] {
        let report = running.reload(parse(to)).unwrap();
        assert!(report.added.is_empty(), "{report:?}");
        assert!(report.removed.is_empty(), "{report:?}");
        assert!(report.swapped.is_empty(), "{report:?}");
        assert!(report.controls_updated.is_empty(), "{report:?}");
        blocks(&slot, 10);
        assert_eq!(running.document().unwrap().main_clock.as_deref(), expected);
    }
    let after = position(&running, "first");
    assert!(
        after > before && after < before + 0.5,
        "the clock played on from {before}, not reset: {after}"
    );
    running.stop();
}
