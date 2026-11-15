//! A development written inline in the loaded invention.

use crate::*;
use fugue::rpc::{DevelopmentSavedIn, EditFailureReason};

fn inline_piece() -> Value {
    // Two instances of one inline development.
    piece(
        json!([{ "name": "voice", "definition": voice(220.0, 330.0) }]),
        &["voice", "voice"],
    )
}

// Criterion: "A move is saved to that development's own document as its
// `layout`: for an inline development, inside the invention's document."
#[test]
fn an_inline_move_is_saved_in_the_invention_document_and_survives_a_save() {
    let mut piece = Piece::from_json(&inline_piece());
    assert!(matches!(
        piece.inside(&["voice"]).saved_in,
        DevelopmentSavedIn::Piece
    ));
    let top_level_before = piece.document().layout.clone();

    piece
        .arrange(
            &["voice"],
            &[("osc_a", Some((120.0, 40.0))), ("mix", Some((300.0, 0.0)))],
        )
        .expect("an inline move applies");

    // get_development shows it at once.
    let inside = piece.inside(&["voice"]);
    assert_eq!(
        placed(&inside.definition.layout, "osc_a"),
        Some((120.0, 40.0))
    );
    assert_eq!(placed(&inside.definition.layout, "mix"), Some((300.0, 0.0)));
    assert_eq!(placed(&inside.definition.layout, "osc_b"), None);

    // The retained document (get_invention) holds it in the definition, and
    // the piece's own layout is untouched.
    let document = piece.document();
    let definition = definition(&document, &["voice"]);
    assert_eq!(
        placements(&definition.layout),
        placements(&inside.definition.layout)
    );
    assert_eq!(placements(&document.layout), placements(&top_level_before));

    // Saved and loaded cold, the arrangement is still there.
    let (_dir, dir) = temp();
    let saved = dir.join("saved.json");
    piece
        .running
        .document_to_save()
        .expect("document retained")
        .save_to_file(&saved)
        .unwrap();
    let reloaded = Piece::from_file(&saved);
    let inside = reloaded.inside(&["voice"]);
    assert_eq!(
        placed(&inside.definition.layout, "osc_a"),
        Some((120.0, 40.0))
    );
    assert_eq!(placed(&inside.definition.layout, "mix"), Some((300.0, 0.0)));
}

// Criterion: "a revision-checked edit the agent can see and repeat" (the
// repeat: positions are absolute, so the same move twice is the same result;
// `null` unplaces, as Tidy and Undo send it).
#[test]
fn an_inline_move_repeats_to_the_same_result_and_null_unplaces() {
    let mut piece = Piece::from_json(&inline_piece());
    let moves = [("osc_b", Some((10.0, 20.0)))];
    piece.arrange(&["voice"], &moves).unwrap();
    let once = placements(&piece.inside(&["voice"]).definition.layout);
    piece
        .arrange(&["voice"], &moves)
        .expect("the same move again applies");
    assert_eq!(
        placements(&piece.inside(&["voice"]).definition.layout),
        once
    );

    piece.arrange(&["voice"], &[("osc_b", None)]).unwrap();
    assert!(piece.inside(&["voice"]).definition.layout.is_empty());
    assert!(definition(&piece.document(), &["voice"]).layout.is_empty());
}

// The nested path `["a","b"]`, the path `get_development` takes.
#[test]
fn a_nested_inline_development_is_arranged_by_its_path() {
    let outer = json!({
        "version": "1.0.0",
        "developments": [ { "name": "b", "definition": voice(110.0, 165.0) } ],
        "modules": [ { "id": "inner", "type": "b" } ],
        "connections": [],
        "outputs": [ { "name": "audio", "from": "inner", "from_port": "audio" } ],
    });
    let mut piece = Piece::from_json(&piece(
        json!([{ "name": "a", "definition": outer }]),
        &["a"],
    ));

    piece
        .arrange(&["a", "b"], &[("osc_b", Some((5.0, 6.0)))])
        .unwrap();
    piece
        .arrange(&["a"], &[("inner", Some((1.0, 2.0)))])
        .unwrap();

    let b = piece.inside(&["a", "b"]);
    assert!(matches!(b.saved_in, DevelopmentSavedIn::Piece));
    assert_eq!(
        placements(&b.definition.layout),
        [("osc_b".to_string(), (5.0, 6.0))]
    );
    let a = piece.inside(&["a"]);
    assert_eq!(
        placements(&a.definition.layout),
        [("inner".to_string(), (1.0, 2.0))]
    );

    let document = piece.document();
    assert_eq!(
        placements(&definition(&document, &["a", "b"]).layout),
        [("osc_b".to_string(), (5.0, 6.0))]
    );
    assert_eq!(
        placements(&definition(&document, &["a"]).layout),
        [("inner".to_string(), (1.0, 2.0))]
    );

    // Failure edges: a name the outer development does not declare, and an
    // id that belongs to the nested definition, not the outer one.
    let error = piece
        .arrange(&["a", "nope"], &[("inner", Some((0.0, 0.0)))])
        .unwrap_err();
    assert_eq!(
        refused_edit(&error),
        (0, EditFailureReason::UnknownDevelopment)
    );
    let error = piece
        .arrange(&["a"], &[("osc_b", Some((0.0, 0.0)))])
        .unwrap_err();
    assert_eq!(refused_edit(&error), (0, EditFailureReason::UnknownModule));
    assert_eq!(
        placements(&piece.inside(&["a"]).definition.layout),
        [("inner".to_string(), (1.0, 2.0))]
    );
}

// Failure edge for the inline path: an unknown development (and a module
// that is not a development) is refused at its index, and the batch is all
// or nothing.
#[test]
fn an_unknown_development_is_refused_at_its_index_with_nothing_applied() {
    let mut piece = Piece::from_json(&inline_piece());
    for path in [&["nope"][..], &["dac"][..]] {
        let edits = [
            inside(&["voice"], &[("osc_a", Some((1.0, 1.0)))]),
            inside(path, &[("osc_a", Some((2.0, 2.0)))]),
        ];
        let error = piece.running.apply_edits(&edits).unwrap_err();
        assert_eq!(
            refused_edit(&error),
            (1, EditFailureReason::UnknownDevelopment),
            "{path:?}"
        );
        assert!(
            piece.inside(&["voice"]).definition.layout.is_empty(),
            "{path:?}"
        );
        assert!(definition(&piece.document(), &["voice"]).layout.is_empty());
    }
    let error = piece
        .arrange(&["voice"], &[("dac", Some((0.0, 0.0)))])
        .unwrap_err();
    assert_eq!(refused_edit(&error), (0, EditFailureReason::UnknownModule));
}
