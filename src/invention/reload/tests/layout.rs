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

    // And back: a placed document reloaded with no layout clears it.
    let report = running.reload(doc(BASE)).unwrap();
    assert!(report.added.is_empty() && report.removed.is_empty() && report.swapped.is_empty());
    assert!(running.document().unwrap().layout.is_empty());
}

#[test]
fn a_layout_never_changes_what_is_heard() {
    let (placed, _) = InventionBuilder::new(48_000)
        .build(placed(&["osc1", "dac"]))
        .unwrap();
    let placed = Settled::start(placed);
    let plain = start(BASE);
    let bits = |(left, right): (Vec<f32>, Vec<f32>)| {
        assert!(left.iter().any(|sample| sample.abs() > 0.01), "silent");
        assert!(right.iter().any(|sample| sample.abs() > 0.01), "silent");
        let bits = |channel: Vec<f32>| channel.into_iter().map(f32::to_bits).collect::<Vec<_>>();
        (bits(left), bits(right))
    };
    assert_eq!(
        bits(placed.pump.render_stereo(256)),
        bits(plain.pump.render_stereo(256))
    );
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

#[test]
fn a_timed_reload_keeps_a_drag_made_while_it_waits() {
    let mut running = start(&placed(&["osc1", "osc2"]).to_json().unwrap());
    // The reload adds a module, places it, and repositions osc2 only.
    let mut next = placed(&["osc1", "osc2"]);
    next.modules.push(ModuleSpec {
        id: "lfo1".into(),
        module_type: "lfo".into(),
        config: serde_json::json!({ "rate": 2.0 }),
    });
    let place = |x: f64| Position { x, y: 9.0 };
    next.layout.modules.insert("lfo1".into(), place(1.0));
    next.layout.modules.insert("osc2".into(), place(2.0));
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    running.reload_at(next, &when, None).expect("queued");

    // Dragged while it waits: osc1, which the reload leaves, and osc2.
    let drag = |id: &str, x: f64| (id.to_string(), Some(Position { x, y: 5.0 }));
    let positions = [drag("osc1", 7.0), drag("osc2", 8.0)].into_iter().collect();
    let moves = [crate::rpc::StructuralEdit::MoveModules { positions }];
    running.apply_edits(&moves).expect("the drag commits");

    running.pump.render(64);
    running.live.adopt_landed();
    let document = running.document().unwrap();
    assert!(
        document.modules.iter().any(|spec| spec.id == "lfo1"),
        "the reload landed"
    );
    let layout = &document.layout.modules;
    assert_eq!(ids(&document), ["lfo1", "osc1", "osc2"]);
    assert_eq!(layout["osc1"], Position { x: 7.0, y: 5.0 });
    assert_eq!(layout["osc2"], place(2.0));
    assert_eq!(layout["lfo1"], place(1.0));
}
