//! Criterion: "A development from an installed package stays read-only here,
//! and the view says why." Simulated with an installed package under
//! `FUGUE_PACKS_DIR` (see `harness::packs_dir`).

use std::path::PathBuf;

use crate::*;
use fugue::rpc::{DevelopmentSavedIn, EditFailureReason};

/// Installs `test.layout.voice` 1.0.0, whose development declares an inline
/// `inner`, and returns its development file.
fn install() -> PathBuf {
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

fn assert_read_only(piece: &mut Piece, path: &[&str], id: &str) {
    let snapshot = piece.inside(path);
    match &snapshot.saved_in {
        DevelopmentSavedIn::ReadOnly { reason } => {
            assert!(!reason.trim().is_empty(), "{path:?} says why")
        }
        other => panic!("{path:?} is read-only, got {other:?}"),
    }
    let error = piece.arrange(path, &[(id, Some((1.0, 2.0)))]).unwrap_err();
    assert_eq!(
        refused_edit(&error),
        (0, EditFailureReason::ReadOnlyDevelopment),
        "{path:?}"
    );
    assert!(piece.inside(path).definition.layout.is_empty(), "{path:?}");
}

#[test]
fn a_package_development_and_anything_inside_it_is_read_only() {
    let file = install();
    let bytes = std::fs::read(&file).unwrap();
    let reference = json!({ "package": "test.layout.voice", "version": "1.0.0" });
    let mut piece = Piece::from_json(&piece(
        json!([{ "name": "shared", "ref": reference }]),
        &["shared"],
    ));

    assert_read_only(&mut piece, &["shared"], "body");
    assert_read_only(&mut piece, &["shared", "inner"], "osc_a");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        bytes,
        "the package is never written"
    );
}

// The design's other read-only home: a file in the package cache, declared
// by path.
#[test]
fn a_file_in_the_package_cache_declared_by_path_is_read_only() {
    let file = install();
    let bytes = std::fs::read(&file).unwrap();
    let (_dir, dir) = temp();
    let doc = piece(
        json!([{ "name": "cached", "path": file.to_str().unwrap() }]),
        &["cached"],
    );
    write_json(&dir.join("piece.json"), &doc);
    let mut piece = Piece::from_file(&dir.join("piece.json"));

    assert_read_only(&mut piece, &["cached"], "body");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        bytes,
        "the cache is never written"
    );
}
