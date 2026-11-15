//! A development that comes from a file the piece can save.

use std::path::Path;

use crate::*;
use fugue::invention::Layout;
use fugue::rpc::DevelopmentSavedIn;

/// A development file in a deliberate hand-written shape: members out of
/// the usual order, odd spacing, numbers written as `110.0`, an existing
/// layout in the middle, and a trailing newline. Only its `layout` may change.
const PAD_LAYOUT: &str = r#"{ "modules": { "osc_a": { "x": 10, "y": 20 } } }"#;

fn pad_text() -> String {
    format!(
        r#"{{
    "title":   "Pad, hand written",
    "version": "1.0.0",
    "connections": [
        {{ "from": "osc_a", "from_port": "audio", "to": "mix", "to_port": "audio.0" }},
        {{ "from": "osc_b", "from_port": "audio", "to": "mix", "to_port": "audio.1" }}
    ],
    "layout": {PAD_LAYOUT},
    "modules": [
        {{ "id": "osc_a", "type": "oscillator", "config": {{ "waveform": "sine", "frequency": 110.0 }} }},
        {{ "id": "osc_b", "type": "oscillator", "config": {{ "waveform": "sine", "frequency": 165.0 }} }},
        {{ "id": "mix",   "type": "mixer",      "config": {{ "channel_count": 2, "levels": [0.3, 0.3] }} }}
    ],
    "outputs": [ {{ "name": "audio", "from": "mix", "from_port": "audio_left" }} ]
}}
"#
    )
}

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn without_layout(mut value: Value) -> Value {
    value.as_object_mut().unwrap().remove("layout");
    value
}

fn layout_of(value: &Value) -> Layout {
    value
        .get("layout")
        .map(|layout| serde_json::from_value(layout.clone()).unwrap())
        .unwrap_or_default()
}

/// A piece in `dir` declaring the pad file, played twice.
fn pad_piece(dir: &Path) -> Piece {
    write_text(&dir.join("pad.json"), &pad_text());
    let doc = piece(
        json!([{ "name": "pad", "path": "pad.json" }]),
        &["pad", "pad"],
    );
    write_json(&dir.join("piece.json"), &doc);
    Piece::from_file(&dir.join("piece.json"))
}

// Criterion: "for a file development, in that file", with the design's file
// rules: only the `layout` member changes, every other byte stays.
#[test]
fn a_file_move_rewrites_only_the_layout_member_of_the_file() {
    let (_dir, dir) = temp();
    let mut piece = pad_piece(&dir);
    let file = dir.join("pad.json");
    let before = read_text(&file);
    let piece_before = read_text(&dir.join("piece.json"));

    let snapshot = piece.inside(&["pad"]);
    match &snapshot.saved_in {
        DevelopmentSavedIn::File { path } => assert_eq!(
            Path::new(path).canonicalize().unwrap(),
            file.canonicalize().unwrap()
        ),
        other => panic!("a file development is saved in its file, got {other:?}"),
    }

    piece
        .arrange(&["pad"], &[("osc_a", None), ("osc_b", Some((300.0, 40.0)))])
        .expect("a file move applies");

    let after = read_text(&file);
    // Every member but the layout parses equal; the layout is the new one.
    assert_eq!(
        without_layout(parsed(&after)),
        without_layout(parsed(&before))
    );
    let layout = layout_of(&parsed(&after));
    assert_eq!(placements(&layout), [("osc_b".to_string(), (300.0, 40.0))]);
    // Every byte outside the layout member is as it was.
    let start = before.find(r#""layout""#).unwrap();
    let end = before.find(PAD_LAYOUT).unwrap() + PAD_LAYOUT.len();
    assert!(
        after.starts_with(&before[..start]),
        "bytes before the layout changed:\n{after}"
    );
    assert!(
        after.ends_with(&before[end..]),
        "bytes after the layout changed:\n{after}"
    );
    // The piece's own file is not written.
    assert_eq!(read_text(&dir.join("piece.json")), piece_before);

    // get_development shows it without a reload; both instances share it.
    let inside = piece.inside(&["pad"]);
    assert_eq!(placements(&inside.definition.layout), placements(&layout));

    // A cold load of the piece reads the moved arrangement from the file.
    let cold = Piece::from_file(&dir.join("piece.json"));
    assert_eq!(
        placements(&cold.inside(&["pad"]).definition.layout),
        placements(&layout)
    );

    // Unplacing everything (Tidy) leaves the file with no placements, the
    // rest still equal.
    drop(cold);
    piece.arrange(&["pad"], &[("osc_b", None)]).unwrap();
    let tidied = read_text(&file);
    assert!(layout_of(&parsed(&tidied)).is_empty(), "{tidied}");
    assert_eq!(
        without_layout(parsed(&tidied)),
        without_layout(parsed(&before))
    );
    assert!(piece.inside(&["pad"]).definition.layout.is_empty());
}

// The file rules: a layout member added to a file with none goes in without
// disturbing the bytes before it, and the file keeps its permissions.
#[cfg(unix)]
#[test]
fn a_file_with_no_layout_gains_one_and_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, dir) = temp();
    let file = dir.join("voice.json");
    let original = serde_json::to_string_pretty(&voice(220.0, 330.0)).unwrap() + "\n";
    write_text(&file, &original);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
    write_json(
        &dir.join("piece.json"),
        &piece(
            json!([{ "name": "voice", "path": "voice.json" }]),
            &["voice"],
        ),
    );
    let mut piece = Piece::from_file(&dir.join("piece.json"));

    piece
        .arrange(&["voice"], &[("mix", Some((-40.5, 12.25)))])
        .unwrap();

    let after = read_text(&file);
    assert_eq!(
        without_layout(parsed(&after)),
        without_layout(parsed(&original))
    );
    assert_eq!(
        placements(&layout_of(&parsed(&after))),
        [("mix".to_string(), (-40.5, 12.25))]
    );
    let body = original.trim_end().strip_suffix('}').unwrap().trim_end();
    assert!(
        after.starts_with(body),
        "bytes before the added layout changed:\n{after}"
    );
    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640, "the file keeps its permissions");
}

// Criterion: "When the same development is used more than once in the
// piece, every use shows the new arrangement" (one file declared under two
// names, each played twice).
#[test]
fn one_file_declared_twice_shows_the_move_under_both_names() {
    let (_dir, dir) = temp();
    let file = dir.join("voice.json");
    write_json(&file, &voice(220.0, 330.0));
    let doc = piece(
        json!([
            { "name": "voice", "path": "voice.json" },
            { "name": "lead", "path": "voice.json" },
        ]),
        &["voice", "voice", "lead", "lead"],
    );
    write_json(&dir.join("piece.json"), &doc);
    let mut piece = Piece::from_file(&dir.join("piece.json"));
    let modules = piece.module_count();

    piece
        .arrange(&["lead"], &[("osc_a", Some((64.0, 32.0)))])
        .unwrap();

    let want = [("osc_a".to_string(), (64.0, 32.0))];
    assert_eq!(placements(&piece.inside(&["lead"]).definition.layout), want);
    assert_eq!(
        placements(&piece.inside(&["voice"]).definition.layout),
        want
    );
    assert_eq!(placements(&layout_of(&parsed(&read_text(&file)))), want);
    assert_eq!(piece.module_count(), modules);
}

// The nested path inside a file: a definition written inline in the file is
// saved in that file.
#[test]
fn a_development_inline_inside_a_file_is_saved_in_that_file() {
    let (_dir, dir) = temp();
    let file = dir.join("band.json");
    let band = json!({
        "version": "1.0.0",
        "developments": [ { "name": "voice", "definition": voice(220.0, 330.0) } ],
        "modules": [ { "id": "inner", "type": "voice" } ],
        "connections": [],
        "outputs": [ { "name": "audio", "from": "inner", "from_port": "audio" } ],
    });
    write_json(&file, &band);
    write_json(
        &dir.join("piece.json"),
        &piece(json!([{ "name": "band", "path": "band.json" }]), &["band"]),
    );
    let mut piece = Piece::from_file(&dir.join("piece.json"));
    assert!(matches!(
        piece.inside(&["band", "voice"]).saved_in,
        DevelopmentSavedIn::File { .. }
    ));

    piece
        .arrange(&["band", "voice"], &[("osc_b", Some((7.0, 8.0)))])
        .unwrap();

    let after = parsed(&read_text(&file));
    let nested = &after["developments"][0]["definition"];
    assert_eq!(
        placements(&layout_of(nested)),
        [("osc_b".to_string(), (7.0, 8.0))]
    );
    // Only that layout changed: the band's own has none, the rest is equal.
    assert!(layout_of(&after).is_empty());
    let mut expected = band.clone();
    expected["developments"][0]["definition"]["layout"] = nested["layout"].clone();
    assert_eq!(after, expected);
    assert_eq!(
        placements(&piece.inside(&["band", "voice"]).definition.layout),
        [("osc_b".to_string(), (7.0, 8.0))]
    );
    assert!(piece.inside(&["band"]).definition.layout.is_empty());
}
