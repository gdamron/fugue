//! Criterion: "A move changes nothing that is heard: no module is rebuilt and
//! the piece does not restart or jump."

use crate::*;

fn max_abs(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |peak, s| peak.max(s.abs()))
}

#[test]
fn moves_inside_inline_and_file_developments_are_sample_identical_to_none() {
    let (_dir, dir) = temp();
    write_json(&dir.join("pad.json"), &voice(110.0, 165.0));
    let doc = piece(
        json!([
            { "name": "voice", "definition": voice(220.0, 330.0) },
            { "name": "pad", "path": "pad.json" },
        ]),
        &["voice", "pad", "voice"],
    );
    write_json(&dir.join("piece.json"), &doc);
    // Both load before either moves, so they play the same document.
    let still = Piece::from_file(&dir.join("piece.json"));
    let mut moved = Piece::from_file(&dir.join("piece.json"));
    let modules = moved.running.full_snapshot();

    let mut heard_still = still.render(40);
    let mut heard_moved = moved.render(40);

    let inline = moved
        .arrange(
            &["voice"],
            &[("osc_a", Some((50.0, 60.0))), ("mix", Some((90.0, 10.0)))],
        )
        .expect("the inline move applies");
    heard_still.extend(still.render(5));
    heard_moved.extend(moved.render(5));
    let file = moved
        .arrange(&["pad"], &[("osc_b", Some((-20.0, 30.0)))])
        .expect("the file move applies");
    // A repeat and an unplacing too, between blocks.
    heard_still.extend(still.render(5));
    heard_moved.extend(moved.render(5));
    moved.arrange(&["pad"], &[("osc_b", None)]).unwrap();
    heard_still.extend(still.render(150));
    heard_moved.extend(moved.render(150));

    for report in [&inline, &file] {
        assert!(report.added.is_empty(), "{report:?}");
        assert!(report.removed.is_empty(), "{report:?}");
        assert!(report.rebuilt.is_empty(), "{report:?}");
        assert!(report.controls_written.is_empty(), "{report:?}");
        assert_eq!(report.connections_added + report.connections_removed, 0);
    }
    assert!(max_abs(&heard_still) > 0.01, "the piece plays something");
    assert_eq!(heard_moved.len(), heard_still.len());
    let first_difference = (heard_moved.iter().zip(&heard_still)).position(|(a, b)| a != b);
    assert_eq!(first_difference, None, "a move is heard");

    // The running graph is the same graph.
    let after = moved.running.full_snapshot();
    assert_eq!(after.modules.len(), modules.modules.len());
    assert_eq!(after.connections.len(), modules.connections.len());
}
