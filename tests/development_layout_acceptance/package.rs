//! A development the piece borrows (from an installed package) is never
//! written: a move inside it is an override in the piece's own layout,
//! layered on the author's arrangement and removed to go back. A piece that
//! is itself an installed package's file stays read-only inside. Simulated
//! with an installed package under `FUGUE_PACKS_DIR` (see
//! `harness::packs_dir`).

use std::path::PathBuf;

use crate::*;
use fugue::rpc::{DevelopmentSavedIn, EditFailureReason};
use fugue::Invention;

/// Installs `test.layout.voice` 1.0.0, whose development declares an inline
/// `inner`, and returns its development file.
pub(crate) fn install() -> PathBuf {
    static INSTALLED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(install_once).clone()
}

fn install_once() -> PathBuf {
    let root = packs_dir().join("test.layout.voice").join("1.0.0");
    let file = root.join("voice.json");
    {
        write_json(
            &root.join("fugue.pkg.json"),
            &json!({ "id": "test.layout.voice", "version": "1.0.0", "kind": "development",
                     "license": "MIT", "authors": [{ "name": "Test" }],
                     "targets": ["external-agent"],
                     "entry": { "development": "voice.json" } }),
        );
        let outer = json!({
            "version": "1.0.0",
            "developments": [ { "name": "inner", "definition": voice(220.0, 330.0) } ],
            "modules": [ { "id": "body", "type": "inner" } ],
            "connections": [],
            "outputs": [ { "name": "audio", "from": "body", "from_port": "audio" } ],
        });
        write_json(&file, &outer);
    }
    file
}

pub(crate) fn shared_piece() -> Value {
    let reference = json!({ "package": "test.layout.voice", "version": "1.0.0" });
    piece(json!([{ "name": "shared", "ref": reference }]), &["shared"])
}

fn assert_override(piece: &Piece, path: &[&str]) {
    let saved_in = piece.inside(path).saved_in;
    assert_eq!(saved_in, DevelopmentSavedIn::Override, "{path:?}");
}

#[test]
fn a_move_inside_a_package_development_is_an_override_in_the_piece() {
    let file = install();
    let bytes = std::fs::read(&file).unwrap();
    let mut piece = Piece::from_json(&shared_piece());
    assert_override(&piece, &["shared"]);
    assert_override(&piece, &["shared", "inner"]);

    piece
        .arrange(&["shared"], &[("body", Some((1.0, 2.0)))])
        .unwrap();
    piece
        .arrange(&["shared", "inner"], &[("osc_a", Some((30.0, -4.0)))])
        .unwrap();

    // Shown on top of the author's (empty) layout, and said to differ.
    let outer = piece.inside(&["shared"]);
    assert_eq!(placed(&outer.definition.layout, "body"), Some((1.0, 2.0)));
    assert_eq!(outer.overrides.keys().collect::<Vec<_>>(), ["body"]);
    let inner = piece.inside(&["shared", "inner"]);
    assert_eq!(
        placed(&inner.definition.layout, "osc_a"),
        Some((30.0, -4.0))
    );
    assert_eq!(placed(&inner.definition.layout, "osc_b"), None);

    // Saved in the piece's own layout, by the path into the development.
    let document = piece.document();
    let keys: Vec<&str> = document
        .layout
        .overrides
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["shared/body", "shared/inner/osc_a"]);
    assert!(document.layout.modules.is_empty());
    assert_eq!(
        std::fs::read(&file).unwrap(),
        bytes,
        "the package is never written"
    );

    // They travel with the piece: saved and loaded elsewhere, the same.
    let (_dir, dir) = temp();
    let saved = dir.join("saved.json");
    piece
        .running
        .document_to_save()
        .unwrap()
        .save_to_file(&saved)
        .unwrap();
    let again = Piece::from_file(&saved);
    let inner = again.inside(&["shared", "inner"]);
    assert_eq!(
        placed(&inner.definition.layout, "osc_a"),
        Some((30.0, -4.0))
    );
    assert!(inner.overrides_dropped.is_empty());
}

#[test]
fn null_removes_an_override_and_back_to_the_original_is_one_edit() {
    install();
    let mut piece = Piece::from_json(&shared_piece());
    let path = ["shared", "inner"];
    let moves = [("osc_a", Some((5.0, 6.0))), ("mix", Some((7.0, 8.0)))];
    piece.arrange(&path, &moves).unwrap();
    // The same move again is the same result.
    piece.arrange(&path, &moves).unwrap();
    assert_eq!(piece.inside(&path).overrides.len(), 2);

    piece.arrange(&path, &[("osc_a", None)]).unwrap();
    let inside = piece.inside(&path);
    assert_eq!(placed(&inside.definition.layout, "osc_a"), None);
    assert_eq!(placed(&inside.definition.layout, "mix"), Some((7.0, 8.0)));

    // Back to the original: every override removed in one edit.
    let ids: Vec<&str> = inside.overrides.keys().map(String::as_str).collect();
    let back: Vec<(&str, Option<(f64, f64)>)> = ids.iter().map(|id| (*id, None)).collect();
    piece.arrange(&path, &back).unwrap();
    let inside = piece.inside(&path);
    assert!(inside.overrides.is_empty());
    assert!(inside.definition.layout.is_empty());
    assert!(piece.document().layout.is_empty());
}

#[test]
fn an_override_move_is_sample_identical_and_rebuilds_nothing() {
    install();
    let still = Piece::from_json(&shared_piece());
    let mut moved = Piece::from_json(&shared_piece());
    let modules = moved.module_count();
    let mut heard_still = still.render(40);
    let mut heard_moved = moved.render(40);
    let report = moved
        .arrange(&["shared", "inner"], &[("osc_b", Some((3.0, 3.0)))])
        .unwrap();
    heard_still.extend(still.render(5));
    heard_moved.extend(moved.render(5));
    moved
        .arrange(&["shared", "inner"], &[("osc_b", None)])
        .unwrap();
    heard_still.extend(still.render(100));
    heard_moved.extend(moved.render(100));
    assert!(
        report.added.is_empty() && report.removed.is_empty(),
        "{report:?}"
    );
    assert!(
        report.rebuilt.is_empty() && report.controls_written.is_empty(),
        "{report:?}"
    );
    assert!(
        heard_still.iter().any(|s| s.abs() > 0.01),
        "the piece plays something"
    );
    let first_difference = (heard_moved.iter().zip(&heard_still)).position(|(a, b)| a != b);
    assert_eq!(first_difference, None, "an override is heard");
    assert_eq!(moved.module_count(), modules);
}

#[test]
fn a_file_in_the_package_cache_declared_by_path_is_arranged_with_overrides() {
    let file = install();
    let bytes = std::fs::read(&file).unwrap();
    let (_dir, dir) = temp();
    let doc = piece(
        json!([{ "name": "cached", "path": file.to_str().unwrap() }]),
        &["cached"],
    );
    write_json(&dir.join("piece.json"), &doc);
    let mut piece = Piece::from_file(&dir.join("piece.json"));
    assert_override(&piece, &["cached"]);
    piece
        .arrange(&["cached"], &[("body", Some((9.0, 9.0)))])
        .unwrap();
    assert!(piece
        .document()
        .layout
        .overrides
        .contains_key("cached/body"));
    assert_eq!(
        std::fs::read(&file).unwrap(),
        bytes,
        "the cache is never written"
    );
}

#[test]
fn a_piece_that_is_an_installed_package_file_stays_read_only_inside() {
    install();
    let root = packs_dir().join("test.layout.example").join("1.0.0");
    let file = root.join("example.json");
    let doc = piece(
        json!([
            { "name": "shared", "ref": { "package": "test.layout.voice", "version": "1.0.0" } },
            { "name": "own", "definition": voice(110.0, 220.0) },
        ]),
        &["shared", "own"],
    );
    write_json(&file, &doc);
    let mut piece = Piece::from_file(&file);
    for (path, id) in [(&["shared"][..], "body"), (&["own"][..], "osc_a")] {
        match piece.inside(path).saved_in {
            DevelopmentSavedIn::ReadOnly { reason } => {
                assert!(reason.contains("installed package"), "{reason}")
            }
            other => panic!("{path:?} is read-only, got {other:?}"),
        }
        let error = piece.arrange(path, &[(id, Some((1.0, 2.0)))]).unwrap_err();
        assert_eq!(
            refused_edit(&error),
            (0, EditFailureReason::ReadOnlyDevelopment)
        );
    }
    assert!(piece.document().layout.is_empty());
}

/// Installs `test.layout.voice` 2.0.0, whose outer module is renamed from
/// `body` to `core`.
pub(crate) fn install_renamed() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(install_renamed_once);
}

fn install_renamed_once() {
    let root = packs_dir().join("test.layout.voice").join("2.0.0");
    write_json(
        &root.join("fugue.pkg.json"),
        &json!({ "id": "test.layout.voice", "version": "2.0.0", "kind": "development",
                 "license": "MIT", "authors": [{ "name": "Test" }],
                 "targets": ["external-agent"],
                 "entry": { "development": "voice.json" } }),
    );
    write_json(
        &root.join("voice.json"),
        &json!({
            "version": "1.0.0",
            "developments": [ { "name": "inner", "definition": voice(220.0, 330.0) } ],
            "modules": [ { "id": "core", "type": "inner" } ],
            "connections": [],
            "outputs": [ { "name": "audio", "from": "core", "from_port": "audio" } ],
        }),
    );
}

fn with_overrides(mut doc: Value, overrides: Value) -> Invention {
    doc["layout"] = json!({ "overrides": overrides });
    serde_json::from_value(doc).unwrap()
}

#[test]
fn a_reload_to_a_new_version_drops_overrides_naming_what_is_gone() {
    install();
    install_renamed();
    let at = json!({ "x": 1, "y": 1 });
    let mut running = Piece::from_json(&shared_piece());
    running
        .arrange(&["shared"], &[("body", Some((1.0, 1.0)))])
        .unwrap();
    running
        .arrange(&["shared", "inner"], &[("mix", Some((2.0, 2.0)))])
        .unwrap();

    let mut moved_on = serde_json::to_value(running.document()).unwrap();
    moved_on["developments"][0]["ref"]["version"] = json!("2.0.0");
    let moved_on: Invention = serde_json::from_value(moved_on).unwrap();
    let report = running.running.reload(moved_on).expect("reloads");
    assert!(
        report.warnings.iter().any(|w| w.contains("'shared/body'")),
        "{:?}",
        report.warnings
    );
    let outer = running.inside(&["shared"]);
    assert_eq!(outer.overrides_dropped, ["body"]);
    assert!(outer.overrides.is_empty());
    let inner = running.inside(&["shared", "inner"]);
    assert_eq!(placed(&inner.definition.layout, "mix"), Some((2.0, 2.0)));
    let keys: Vec<String> = running.document().layout.overrides.into_keys().collect();
    assert_eq!(keys, ["shared/inner/mix"]);

    // A load drops them too, and an override for a development the piece no
    // longer uses, or one it owns.
    let doc = with_overrides(
        piece(
            json!([
                { "name": "shared", "ref": { "package": "test.layout.voice", "version": "2.0.0" } },
                { "name": "own", "definition": voice(1.0, 2.0) },
            ]),
            &["shared", "own"],
        ),
        json!({ "shared/body": at, "shared/core": at, "gone/osc": at, "own/osc_a": at }),
    );
    let (runtime, _) = fugue::InventionBuilder::new(RATE)
        .build(doc.clone())
        .unwrap();
    let warnings = runtime.load_warnings();
    for key in ["shared/body", "gone/osc", "own/osc_a"] {
        assert!(
            warnings.iter().any(|w| w.contains(key)),
            "{key}: {warnings:?}"
        );
    }
    drop(runtime);
    let loaded = Piece::from_json(&serde_json::to_value(doc).unwrap());
    let kept: Vec<String> = loaded.document().layout.overrides.into_keys().collect();
    assert_eq!(kept, ["shared/core"]);
    assert_eq!(loaded.inside(&["shared"]).overrides_dropped, ["body"]);
}

#[test]
fn a_timed_reload_lands_its_own_overrides_and_keeps_one_made_while_it_waits() {
    install();
    let mut running = Piece::from_json(&shared_piece());
    running
        .arrange(&["shared"], &[("body", Some((1.0, 1.0)))])
        .unwrap();
    // The reload moves one override, removes none, adds one, and adds a
    // module, so it is a structural change that waits to land.
    let mut next = serde_json::to_value(running.document()).unwrap();
    next["layout"]["overrides"] = json!({
        "shared/body": { "x": 4.0, "y": 4.0 },
        "shared/inner/osc_b": { "x": 5.0, "y": 5.0 },
    });
    next["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "lfo", "type": "lfo" }));
    let next: Invention = serde_json::from_value(next).unwrap();
    let when = fugue::rpc::EditWhen::AfterSamples { samples: 2048 };
    running
        .running
        .reload_at(next, &when, None)
        .expect("queued");
    // An override made while it waits.
    running
        .arrange(&["shared", "inner"], &[("mix", Some((6.0, 6.0)))])
        .unwrap();
    running.render(64);

    let document = running.document();
    assert!(document.modules.iter().any(|m| m.id == "lfo"), "landed");
    let overrides: Vec<(&str, f64)> = (document.layout.overrides.iter())
        .map(|(key, at)| (key.as_str(), at.x))
        .collect();
    assert_eq!(
        overrides,
        [
            ("shared/body", 4.0),
            ("shared/inner/mix", 6.0),
            ("shared/inner/osc_b", 5.0)
        ]
    );
    let inner = running.inside(&["shared", "inner"]);
    assert_eq!(placed(&inner.definition.layout, "osc_b"), Some((5.0, 5.0)));
}
