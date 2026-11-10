//! A document's layout on load and reload: presentation only, so a
//! difference in it alone rebuilds nothing, and a position that places no
//! module is dropped with a warning.

use super::*;
use crate::invention::layout::Position;

/// BASE with a layout placing `placed` modules at (index, 0).
fn placed(placed: &[&str]) -> Invention {
    let mut document = doc(BASE);
    for (index, id) in placed.iter().enumerate() {
        let position = Position {
            x: index as f64,
            y: 0.0,
        };
        document.layout.modules.insert(id.to_string(), position);
    }
    document
}

fn ids(document: &Invention) -> Vec<String> {
    document.layout.modules.keys().cloned().collect()
}

#[test]
fn a_layout_only_reload_rebuilds_nothing_and_keeps_the_new_layout() {
    let mut running = start(BASE);
    let before = running.pump.render(4);
    let generation = running.live.generation();

    let report = running.reload(placed(&["dac", "osc1"])).unwrap();

    assert!(report.added.is_empty() && report.removed.is_empty() && report.swapped.is_empty());
    assert!(report.controls_updated.is_empty());
    assert_eq!(report.connections_added + report.connections_removed, 0);
    assert_eq!(report.unchanged, 3);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(running.live.generation(), generation);
    // The oscillators kept their phase: the output carries on.
    let (fresh, _) = InventionBuilder::new(48_000).build(doc(BASE)).unwrap();
    let fresh = Settled::start(fresh);
    let mut expected = fresh.pump.render(8);
    assert_eq!(&expected[..before.len()], &before[..]);
    let rest = expected.split_off(before.len());
    assert_eq!(running.pump.render(4), rest);
    assert_eq!(ids(&running.document().unwrap()), ["dac", "osc1"]);
}

#[test]
fn a_reload_omitting_a_module_drops_its_position_with_a_warning() {
    let mut running = start(&placed(&["osc1", "osc2"]).to_json().unwrap());
    // osc2 is gone from the new document, but its layout still names it.
    let mut next = placed(&["osc1", "osc2"]);
    next.modules.retain(|spec| spec.id != "osc2");
    next.connections.retain(|conn| conn.from != "osc2");

    let report = running.reload(next).unwrap();

    assert_eq!(report.removed, ["osc2"]);
    assert_eq!(ids(&running.document().unwrap()), ["osc1"]);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("'osc2'"),
        "{:?}",
        report.warnings
    );
}

#[test]
fn a_load_ignores_a_position_for_an_unknown_module_with_a_warning() {
    let document = placed(&["osc1", "ghost"]);
    let (runtime, _) = InventionBuilder::new(48_000).build(document).unwrap();
    let state = runtime.state.lock().unwrap();
    let warnings: Vec<String> = state
        .load_warnings
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("'ghost'"), "{warnings:?}");
    assert_eq!(ids(state.document.as_ref().unwrap()), ["osc1"]);
}

/// No checked-in example re-saves byte for byte (the save format's own
/// spacing differs from the hand-written files), so this compares a save of
/// a loaded and played example against a save of the file as parsed, and
/// re-saves the result to check it is a fixed point.
#[test]
fn an_example_without_a_layout_saves_as_it_did_before() {
    let path = "examples/lfo_vibrato.json";
    let parsed = Invention::from_file(path).unwrap();
    assert!(parsed.layout.is_empty());
    let (runtime, _) = InventionBuilder::new(48_000).build(parsed.clone()).unwrap();
    let running = Settled::start(runtime);
    running.pump.render(4);

    let dir = tempfile::tempdir().unwrap();
    let (played, direct) = (
        dir.path().join("played.json"),
        dir.path().join("direct.json"),
    );
    running.document().unwrap().save_to_file(&played).unwrap();
    parsed.save_to_file(&direct).unwrap();
    let saved = std::fs::read(&played).unwrap();
    assert_eq!(saved, std::fs::read(&direct).unwrap());
    assert!(!String::from_utf8_lossy(&saved).contains("layout"));

    let again = dir.path().join("again.json");
    Invention::from_file(played.to_str().unwrap())
        .unwrap()
        .save_to_file(&again)
        .unwrap();
    assert_eq!(std::fs::read(&again).unwrap(), saved);
}
