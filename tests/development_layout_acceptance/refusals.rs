//! Refusals: nothing written, nothing in the running piece changed.

use std::path::Path;

use crate::*;
use fugue::rpc::{EditFailureReason, EditWhen, StructuralEdit};
use fugue::RpcErrorCode;

/// A piece in `dir` with an inline `voice` and a file `pad` (`pad.json`),
/// and a second file `lead` (`lead.json`).
fn mixed_piece(dir: &Path) -> Piece {
    write_json(&dir.join("pad.json"), &voice(110.0, 165.0));
    write_json(&dir.join("lead.json"), &voice(440.0, 660.0));
    let doc = piece(
        json!([
            { "name": "voice", "definition": voice(220.0, 330.0) },
            { "name": "pad", "path": "pad.json" },
            { "name": "lead", "path": "lead.json" },
        ]),
        &["voice", "pad", "lead"],
    );
    write_json(&dir.join("piece.json"), &doc);
    Piece::from_file(&dir.join("piece.json"))
}

/// One development's path and its modules' positions.
type Inside = (Vec<&'static str>, Vec<(String, (f64, f64))>);

/// Everything a refusal must leave as it was.
struct Untouched {
    files: Vec<(std::path::PathBuf, Option<Vec<u8>>)>,
    insides: Vec<Inside>,
    document: String,
    modules: usize,
}

impl Untouched {
    fn record(piece: &Piece, dir: &Path) -> Untouched {
        let files = ["pad.json", "lead.json", "piece.json"]
            .iter()
            .map(|name| (dir.join(name), std::fs::read(dir.join(name)).ok()))
            .collect();
        let insides = [vec!["voice"], vec!["pad"], vec!["lead"]]
            .into_iter()
            .map(|path| {
                let layout = placements(&piece.inside(&path).definition.layout);
                (path, layout)
            })
            .collect();
        Untouched {
            files,
            insides,
            document: piece.document().to_json().unwrap(),
            modules: piece.module_count(),
        }
    }

    fn check(&self, piece: &Piece, label: &str) {
        for (path, bytes) in &self.files {
            assert_eq!(
                &std::fs::read(path).ok(),
                bytes,
                "{label}: {} changed",
                path.display()
            );
        }
        for (path, layout) in &self.insides {
            let now = placements(&piece.inside(path).definition.layout);
            assert_eq!(&now, layout, "{label}: {path:?} moved");
        }
        assert_eq!(
            piece.document().to_json().unwrap(),
            self.document,
            "{label}: document"
        );
        assert_eq!(piece.module_count(), self.modules, "{label}: modules");
    }
}

// Criterion: "A file that cannot be written is refused with a reason, and
// the arrangement ... returns to what is saved."
#[cfg(unix)]
#[test]
fn a_read_only_file_is_refused_and_nothing_changes() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, dir) = temp();
    let mut piece = mixed_piece(&dir);
    let file = dir.join("pad.json");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    if std::fs::OpenOptions::new().write(true).open(&file).is_ok() {
        eprintln!("skipped: this user can write a read-only file (root?)");
        return;
    }
    let untouched = Untouched::record(&piece, &dir);

    let error = piece
        .arrange(&["pad"], &[("osc_a", Some((1.0, 2.0)))])
        .unwrap_err();
    assert_eq!(
        refused_edit(&error),
        (0, EditFailureReason::DevelopmentNotWritten)
    );
    assert!(!error.message.trim().is_empty(), "the refusal says why");
    untouched.check(&piece, "read-only file");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o444);
}

// A compressed (.fyug) development file loads, but its layout is written
// back as JSON only: it is refused with what to do, and left as it was.
#[test]
fn a_compressed_file_is_refused_with_how_to_arrange_it() {
    use std::io::Write;

    let (_dir, dir) = temp();
    let file = dir.join("pad.json");
    let text = serde_json::to_string_pretty(&voice(110.0, 165.0)).unwrap();
    let mut packed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    packed.write_all(text.as_bytes()).unwrap();
    let mut piece = mixed_piece(&dir);
    std::fs::write(&file, packed.finish().unwrap()).unwrap();
    let document = fugue::Invention::from_file(dir.join("piece.json").to_str().unwrap()).unwrap();
    piece
        .running
        .reload(document)
        .expect("a compressed development loads");
    let untouched = Untouched::record(&piece, &dir);

    let error = piece
        .arrange(&["pad"], &[("osc_a", Some((1.0, 2.0)))])
        .unwrap_err();
    assert_eq!(
        refused_edit(&error),
        (0, EditFailureReason::DevelopmentNotWritten)
    );
    assert!(
        error.message.contains("save it uncompressed"),
        "{}",
        error.message
    );
    untouched.check(&piece, "compressed file");
}

// The design's other unwritable file: one changed on disk since the piece
// loaded it, or gone. Reloading takes the change and the move then applies.
#[test]
fn a_file_changed_or_gone_since_load_is_refused_until_reloaded() {
    let (_dir, dir) = temp();
    let mut piece = mixed_piece(&dir);
    let file = dir.join("pad.json");
    let mut edited = voice(110.0, 165.0);
    edited["modules"][0]["config"]["frequency"] = json!(123.0);
    write_json(&file, &edited);
    let untouched = Untouched::record(&piece, &dir);

    let error = piece
        .arrange(&["pad"], &[("osc_a", Some((1.0, 2.0)))])
        .unwrap_err();
    assert_eq!(
        refused_edit(&error),
        (0, EditFailureReason::DevelopmentNotWritten)
    );
    untouched.check(&piece, "changed on disk");

    let lead = dir.join("lead.json");
    std::fs::remove_file(&lead).unwrap();
    let untouched = Untouched::record(&piece, &dir);
    let error = piece
        .arrange(&["lead"], &[("osc_a", Some((1.0, 2.0)))])
        .unwrap_err();
    assert_eq!(
        refused_edit(&error),
        (0, EditFailureReason::DevelopmentNotWritten)
    );
    untouched.check(&piece, "gone");
    assert!(!lead.exists(), "a file that is gone is not recreated");

    // Reload to take the change, then move again.
    write_json(&lead, &voice(440.0, 660.0));
    let document = fugue::Invention::from_file(dir.join("piece.json").to_str().unwrap()).unwrap();
    piece.running.reload(document).expect("reloads");
    piece
        .arrange(&["pad"], &[("osc_a", Some((1.0, 2.0)))])
        .expect("applies after reload");
    let on_disk: Value = serde_json::from_str(&read_text(&file)).unwrap();
    assert_eq!(on_disk["modules"][0]["config"]["frequency"], json!(123.0));
    assert_eq!(
        placements(&piece.inside(&["pad"]).definition.layout),
        [("osc_a".to_string(), (1.0, 2.0))]
    );
}

// Design: "It applies at once, in a batch of `move_modules` and
// `name_modules` alone. With any structural edit, or timed (`when`), it is
// refused `invalid_request`, nothing applied."
#[test]
fn a_move_inside_mixed_with_a_structural_edit_is_refused_with_nothing_applied() {
    let (_dir, dir) = temp();
    let mut piece = mixed_piece(&dir);
    let untouched = Untouched::record(&piece, &dir);
    let add = StructuralEdit::AddModule {
        id: "extra".into(),
        module_type: "oscillator".into(),
        config: json!({ "frequency": 99.0 }),
        name: None,
    };
    for path in [&["voice"][..], &["pad"][..]] {
        for edits in [
            vec![inside(path, &[("osc_a", Some((1.0, 2.0)))]), add.clone()],
            vec![add.clone(), inside(path, &[("osc_a", Some((1.0, 2.0)))])],
        ] {
            let error = piece.running.apply_edits(&edits).unwrap_err();
            assert_eq!(
                error.code,
                RpcErrorCode::InvalidRequest,
                "{path:?}: {error:?}"
            );
            untouched.check(&piece, &format!("mixed {path:?}"));
        }
    }

    // Moves and names together are still one presentation batch.
    let names = StructuralEdit::NameModules {
        names: [("p0".to_string(), Some("Low voice".to_string()))].into(),
    };
    let edits = [
        top_level(&[("dac", Some((500.0, 0.0)))]),
        inside(&["voice"], &[("osc_a", Some((1.0, 2.0)))]),
        names,
    ];
    piece
        .running
        .apply_edits(&edits)
        .expect("moves and names apply together");
    assert_eq!(
        placed(&piece.inside(&["voice"]).definition.layout, "osc_a"),
        Some((1.0, 2.0))
    );
    assert_eq!(placed(&piece.document().layout, "dac"), Some((500.0, 0.0)));
}

#[test]
fn a_timed_move_inside_is_refused_with_nothing_applied() {
    let (_dir, dir) = temp();
    let mut piece = mixed_piece(&dir);
    let untouched = Untouched::record(&piece, &dir);
    for path in [&["voice"][..], &["pad"][..]] {
        let edits = [inside(path, &[("osc_a", Some((1.0, 2.0)))])];
        let when = EditWhen::AfterSamples { samples: 4_800 };
        let error = piece
            .running
            .apply_edits_at(&edits, &when, None)
            .unwrap_err();
        assert_eq!(
            error.code,
            RpcErrorCode::InvalidRequest,
            "{path:?}: {error:?}"
        );
        piece.render(100);
        untouched.check(&piece, &format!("timed {path:?}"));
    }
}

// Design: "One batch writes at most one development file (`invalid_request`
// otherwise), so a batch stays all or nothing."
#[test]
fn a_batch_writing_two_files_is_refused_with_neither_written() {
    let (_dir, dir) = temp();
    let mut piece = mixed_piece(&dir);
    let untouched = Untouched::record(&piece, &dir);
    let edits = [
        inside(&["pad"], &[("osc_a", Some((1.0, 2.0)))]),
        inside(&["lead"], &[("osc_a", Some((3.0, 4.0)))]),
    ];
    let error = piece.running.apply_edits(&edits).unwrap_err();
    assert_eq!(error.code, RpcErrorCode::InvalidRequest, "{error:?}");
    untouched.check(&piece, "two files");
}
