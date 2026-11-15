//! More of what a piece borrows: going back to the original one development
//! at a time, refusals that change nothing, overrides that travel with a
//! saved piece and that an older core ignores, and the view's wire shape.
//! Uses the installed `test.layout.voice` from `package`, and
//! `test.layout.authored`, whose author placed some of its modules.

use crate::package::{install, install_renamed, shared_piece};
use crate::*;
use fugue::rpc::{DevelopmentSavedIn, EditFailureReason};
use fugue::Invention;

/// Installs `test.layout.authored` 1.0.0: the voice with the author's
/// positions for `osc_a` and `mix` (`osc_b` has none).
fn install_authored() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(|| {
        let root = packs_dir().join("test.layout.authored").join("1.0.0");
        write_json(
            &root.join("fugue.pkg.json"),
            &json!({ "id": "test.layout.authored", "version": "1.0.0", "kind": "development",
                     "license": "MIT", "authors": [{ "name": "Test" }],
                     "targets": ["external-agent"],
                     "entry": { "development": "voice.json" } }),
        );
        let mut voice = voice(220.0, 330.0);
        voice["layout"] = json!({ "modules": {
            "osc_a": { "x": 1.0, "y": 1.0 }, "mix": { "x": 5.0, "y": 5.0 } } });
        write_json(&root.join("voice.json"), &voice);
    });
}

fn voice_ref() -> Value {
    json!({ "package": "test.layout.voice", "version": "1.0.0" })
}

fn authored_ref() -> Value {
    json!({ "package": "test.layout.authored", "version": "1.0.0" })
}

/// Where the author put the authored voice's modules.
fn author_placements() -> Vec<(String, (f64, f64))> {
    vec![("mix".into(), (5.0, 5.0)), ("osc_a".into(), (1.0, 1.0))]
}

/// The piece's overrides as (key, (x, y)), in key order.
fn overrides(piece: &Piece) -> Vec<(String, (f64, f64))> {
    (piece.document().layout.overrides.into_iter())
        .map(|(key, at)| (key, (at.x, at.y)))
        .collect()
}

fn saved_json(piece: &Piece) -> String {
    let saved = piece.running.document_to_save().expect("document retained");
    saved.to_json().unwrap()
}

#[test]
fn back_to_the_original_removes_one_developments_overrides_and_undo_restores_them() {
    install_authored();
    let doc = piece(
        json!([{ "name": "first", "ref": authored_ref() }, { "name": "second", "ref": authored_ref() }]),
        &["first", "second"],
    );
    let mut piece = Piece::from_json(&doc);
    let path = ["first"];
    let moves = [
        ("osc_a", Some((50.0, 60.0))),
        ("osc_b", Some((7.0, 8.0))),
        ("mix", Some((9.0, 9.0))),
    ];
    piece.arrange(&path, &moves).unwrap();
    piece
        .arrange(&["second"], &[("osc_a", Some((20.0, 21.0)))])
        .unwrap();
    // The same package under two names is arranged apart.
    assert_eq!(piece.inside(&path).overrides.len(), 3);
    assert_eq!(piece.inside(&["second"]).overrides.len(), 1);

    // Back to the original: every listed override unset in one edit.
    let earlier = piece.inside(&path).overrides;
    let back: Vec<(&str, Option<(f64, f64)>)> =
        earlier.keys().map(|id| (id.as_str(), None)).collect();
    piece.arrange(&path, &back).unwrap();
    assert!(piece.inside(&path).overrides.is_empty());
    let shown = piece.inside(&path).definition.layout;
    assert_eq!(
        placements(&shown),
        author_placements(),
        "the author's again"
    );
    let kept = [("second/osc_a".to_string(), (20.0, 21.0))];
    assert_eq!(overrides(&piece), kept, "the other keeps its arrangement");
    assert!(!saved_json(&piece).contains("first/"));

    // Undo is the earlier positions again.
    let again: Vec<(&str, Option<(f64, f64)>)> = (earlier.iter())
        .map(|(id, at)| (id.as_str(), Some((at.x, at.y))))
        .collect();
    piece.arrange(&path, &again).unwrap();
    assert_eq!(piece.inside(&path).overrides, earlier);

    // With none left, no override table is saved.
    piece.arrange(&path, &back).unwrap();
    piece.arrange(&["second"], &[("osc_a", None)]).unwrap();
    assert!(!saved_json(&piece).contains("overrides"));
}

#[test]
fn a_module_the_borrowed_development_lacks_is_refused_and_nothing_changes() {
    install();
    let mut piece = Piece::from_json(&shared_piece());
    piece
        .arrange(&["shared"], &[("body", Some((1.0, 2.0)))])
        .unwrap();
    let before = serde_json::to_value(piece.document()).unwrap();
    let edits = [
        inside(&["shared", "inner"], &[("osc_b", Some((3.0, 3.0)))]),
        inside(&["shared"], &[("nope", Some((4.0, 4.0)))]),
    ];
    let error = piece.running.apply_edits(&edits).unwrap_err();
    assert_eq!(refused_edit(&error), (1, EditFailureReason::UnknownModule));
    assert_eq!(serde_json::to_value(piece.document()).unwrap(), before);
    assert!(piece.inside(&["shared", "inner"]).overrides.is_empty());
}

#[test]
fn a_reload_without_a_borrowed_development_drops_its_overrides() {
    install();
    let doc = piece(
        json!([{ "name": "first", "ref": voice_ref() }, { "name": "second", "ref": voice_ref() }]),
        &["first", "second"],
    );
    let mut piece = Piece::from_json(&doc);
    piece
        .arrange(&["first"], &[("body", Some((1.0, 2.0)))])
        .unwrap();
    piece
        .arrange(&["second"], &[("body", Some((3.0, 4.0)))])
        .unwrap();

    // Without `second`, its instance, and the bus channel it fed.
    let mut next = serde_json::to_value(piece.document()).unwrap();
    next["developments"].as_array_mut().unwrap().pop();
    next["modules"]
        .as_array_mut()
        .unwrap()
        .retain(|m| m["id"] != "p1");
    for module in next["modules"].as_array_mut().unwrap() {
        if module["id"] == "bus" {
            module["config"] = json!({ "channel_count": 1, "levels": [0.5] });
        }
    }
    next["connections"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["from"] != "p1");
    let report = piece
        .running
        .reload(serde_json::from_value(next).unwrap())
        .expect("reloads");
    let named = |key: &str| report.warnings.iter().any(|w| w.contains(key));
    assert!(named("'second/body'"), "{:?}", report.warnings);
    assert!(!named("'first/body'"), "{:?}", report.warnings);
    assert_eq!(overrides(&piece), [("first/body".to_string(), (1.0, 2.0))]);
}

#[test]
fn the_same_piece_saved_outside_the_packages_directory_is_arranged() {
    let file = install();
    let doc = piece(
        json!([
            { "name": "own", "definition": voice(220.0, 330.0) },
            { "name": "shared", "ref": voice_ref() },
            { "name": "cached", "path": file.to_str().unwrap() },
        ]),
        &["own", "shared", "cached"],
    );
    let (_dir, dir) = temp();
    write_json(&dir.join("copy.json"), &doc);
    let mut piece = Piece::from_file(&dir.join("copy.json"));
    assert_eq!(piece.inside(&["own"]).saved_in, DevelopmentSavedIn::Piece);
    for path in [&["shared"][..], &["cached"][..]] {
        assert_eq!(piece.inside(path).saved_in, DevelopmentSavedIn::Override);
    }
    piece
        .arrange(&["own"], &[("osc_a", Some((1.0, 2.0)))])
        .unwrap();
    piece
        .arrange(&["shared"], &[("body", Some((1.0, 2.0)))])
        .unwrap();
}

#[test]
fn overrides_travel_with_a_saved_piece_to_a_cold_load_elsewhere() {
    install();
    install_authored();
    let doc = piece(
        json!([
            { "name": "shared", "ref": { "package": "test.layout.voice", "version": "1.0.0" } },
            { "name": "authored", "ref": authored_ref() },
        ]),
        &["shared", "authored"],
    );
    let mut piece = Piece::from_json(&doc);
    piece
        .arrange(&["shared"], &[("body", Some((50.0, 60.0)))])
        .unwrap();
    piece
        .arrange(&["shared", "inner"], &[("mix", Some((11.0, 12.0)))])
        .unwrap();
    let moves = [("osc_a", Some((30.0, 31.0))), ("osc_b", Some((32.0, 33.0)))];
    piece.arrange(&["authored"], &moves).unwrap();
    let (_dir, dir) = temp();
    let saved = dir.join("elsewhere").join("saved.json");
    let document = piece.running.document_to_save().unwrap();
    document.save_to_file(&saved).unwrap();

    let mut loaded = Piece::from_file(&saved);
    assert!(loaded.load_warnings.is_empty());
    assert_eq!(overrides(&loaded), overrides(&piece));
    for path in [&["shared"][..], &["shared", "inner"][..], &["authored"][..]] {
        let (here, there) = (piece.inside(path), loaded.inside(path));
        assert_eq!(there.saved_in, DevelopmentSavedIn::Override);
        assert_eq!(there.definition.layout, here.definition.layout, "{path:?}");
        assert_eq!(there.overrides, here.overrides, "{path:?}");
    }
    // Still layered there, the author's untouched `mix` kept: removing the
    // overrides goes back to the author's.
    let shown = loaded.inside(&["authored"]).definition.layout;
    assert_eq!(placed(&shown, "mix"), Some((5.0, 5.0)));
    assert_eq!(placed(&shown, "osc_a"), Some((30.0, 31.0)));
    loaded
        .arrange(&["authored"], &[("osc_a", None), ("osc_b", None)])
        .unwrap();
    let shown = loaded.inside(&["authored"]).definition.layout;
    assert_eq!(placements(&shown), author_placements());
}

/// What an older core does with a piece that has overrides: its layout
/// ignores a member it does not know. Here a member this core does not know
/// stands in for `overrides`, and neither changes what is heard.
#[test]
fn an_unknown_layout_member_is_ignored_and_overrides_sound_like_none() {
    install();
    let plain = shared_piece();
    let mut later = plain.clone();
    later["layout"] = json!({
        "modules": { "p0": { "x": 1.0, "y": 2.0 } },
        "overrides": { "shared/body": { "x": 3.0, "y": 4.0 } },
        "from_a_later_core": { "anything": [1, 2, 3] },
    });
    let arranged = Piece::from_json(&later);
    assert!(arranged.load_warnings.is_empty());
    assert_eq!(placed(&arranged.document().layout, "p0"), Some((1.0, 2.0)));
    let heard = arranged.render(120);
    assert!(heard.iter().any(|s| s.abs() > 0.01), "it plays");
    assert_eq!(heard, Piece::from_json(&plain).render(120));
}

#[test]
fn a_view_carries_overrides_and_dropped_on_the_wire_only_when_there_are_some() {
    install();
    install_renamed();
    let mut piece = Piece::from_json(&shared_piece());
    let wire = serde_json::to_value(piece.inside(&["shared"])).unwrap();
    assert_eq!(wire["saved_in"], json!({ "kind": "override" }));
    assert!(wire.get("overrides").is_none(), "{wire}");
    assert!(wire.get("overrides_dropped").is_none(), "{wire}");

    piece
        .arrange(&["shared"], &[("body", Some((1.0, 2.0)))])
        .unwrap();
    let wire = serde_json::to_value(piece.inside(&["shared"])).unwrap();
    assert_eq!(wire["overrides"], json!({ "body": { "x": 1.0, "y": 2.0 } }));
    assert!(wire.get("overrides_dropped").is_none(), "{wire}");

    let mut next = serde_json::to_value(piece.document()).unwrap();
    next["developments"][0]["ref"]["version"] = json!("2.0.0");
    let next: Invention = serde_json::from_value(next).unwrap();
    piece.running.reload(next).expect("reloads");
    let wire = serde_json::to_value(piece.inside(&["shared"])).unwrap();
    assert_eq!(wire["overrides_dropped"], json!(["body"]));
    assert!(wire.get("overrides").is_none(), "{wire}");
}
