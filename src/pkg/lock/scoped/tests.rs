use super::*;
use crate::pkg::{compute_integrity, LockSource, LockedPackage, LOCKFILE_NAME};
use serde_json::json;
use std::fs;

/// Installs `id@1.0.0` under `packs` and returns its locked entry.
fn install(packs: &Path, id: &str, dependencies: &[&str]) -> LockedPackage {
    let dir = packs.join(id).join("1.0.0");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("sample.wav"), id).unwrap();
    LockedPackage {
        version: "1.0.0".into(),
        kind: "sample-pack".into(),
        source: LockSource::Registry {
            id: id.into(),
            version: "1.0.0".into(),
        },
        integrity: compute_integrity(&dir).unwrap(),
        path: dir,
        dependencies: dependencies.iter().map(|dep| dep.to_string()).collect(),
    }
}

fn corrupt(packs: &Path, id: &str) {
    fs::write(packs.join(id).join("1.0.0/sample.wav"), "tampered").unwrap();
}

/// A lockfile beside `dir/main.json` locking `used` (which depends on `dep`)
/// and `unused`, plus an invention that plays only `used`.
fn setup(dir: &Path) -> (Vec<PathBuf>, PathBuf, Invention) {
    let packs = dir.join("packs");
    let mut lock = Lockfile::new();
    lock.upsert(
        "fugue.demo.used",
        install(&packs, "fugue.demo.used", &["fugue.demo.dep@1.0.0"]),
    );
    lock.upsert("fugue.demo.dep", install(&packs, "fugue.demo.dep", &[]));
    lock.upsert(
        "fugue.demo.unused",
        install(&packs, "fugue.demo.unused", &[]),
    );
    let lock_path = dir.join(LOCKFILE_NAME);
    fs::write(&lock_path, lock.to_bytes().unwrap()).unwrap();

    let mut invention: Invention = serde_json::from_value(json!({
        "modules": [{"id": "s", "type": "sample_player",
            "config": {"asset": "fugue.demo.used@1.0.0:sample.wav"}}],
        "connections": []
    }))
    .unwrap();
    invention.source_path = Some(dir.join("main.json"));
    (vec![lock_path], packs, invention)
}

#[test]
fn dependency_closure_follows_locked_edges() {
    let tmp = tempfile::tempdir().unwrap();
    let (candidates, _, _) = setup(tmp.path());
    let lock = Lockfile::read(&candidates[0]).unwrap();
    let closure = lock.dependency_closure(["fugue.demo.used", "not.locked"]);
    assert_eq!(
        closure.into_iter().collect::<Vec<_>>(),
        ["fugue.demo.dep", "fugue.demo.used", "not.locked"]
    );
}

#[test]
fn frozen_load_ignores_a_corrupted_unused_package() {
    let tmp = tempfile::tempdir().unwrap();
    let (candidates, packs, invention) = setup(tmp.path());
    corrupt(&packs, "fugue.demo.unused");
    Lockfile::verify_first_for_invention(&candidates, &packs, &invention).unwrap();
    // The all-packages check still sees it.
    let all = Lockfile::verify_first(&candidates, &packs).unwrap_err();
    assert!(all
        .to_string()
        .contains("fugue.demo.unused@1.0.0 integrity mismatch"));
}

#[test]
fn frozen_load_rejects_a_corrupted_used_package_or_dependency() {
    for id in ["fugue.demo.used", "fugue.demo.dep"] {
        let tmp = tempfile::tempdir().unwrap();
        let (candidates, packs, invention) = setup(tmp.path());
        corrupt(&packs, id);
        let err = Lockfile::verify_first_for_invention(&candidates, &packs, &invention)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(&format!("{id}@1.0.0 integrity mismatch")),
            "{err}"
        );
    }
}

#[test]
fn frozen_load_rejects_a_corrupted_package_chosen_only_through_an_alias() {
    let tmp = tempfile::tempdir().unwrap();
    let (candidates, packs, mut invention) = setup(tmp.path());
    // Codex's repro: the development exposes its sample player's `asset` as
    // `sample`, and only the instance config names the package.
    invention.modules = serde_json::from_value(json!([{"id": "d", "type": "drums",
        "config": {"sample": "fugue.demo.unused@1:sample.wav"}}]))
    .unwrap();
    invention.developments = serde_json::from_value(json!([{"name": "drums", "definition": {
        "modules": [{"id": "s", "type": "sample_player", "config": {}}], "connections": [],
        "controls": [{"name": "sample", "module": "s", "control": "asset"}]}}]))
    .unwrap();
    corrupt(&packs, "fugue.demo.unused");
    let err = Lockfile::verify_first_for_invention(&candidates, &packs, &invention)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("fugue.demo.unused@1.0.0 integrity mismatch"),
        "{err}"
    );
}

#[test]
fn an_unknown_used_set_verifies_every_package() {
    let tmp = tempfile::tempdir().unwrap();
    let (candidates, packs, mut invention) = setup(tmp.path());
    invention.developments =
        serde_json::from_value(json!([{"name": "gone", "path": "missing.json"}])).unwrap();
    corrupt(&packs, "fugue.demo.unused");
    let err = Lockfile::verify_first_for_invention(&candidates, &packs, &invention)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("fugue.demo.unused@1.0.0 integrity mismatch"),
        "{err}"
    );
}

#[test]
fn no_lockfile_is_nothing_to_enforce() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, packs, invention) = setup(tmp.path());
    let missing = [tmp.path().join("nope").join(LOCKFILE_NAME)];
    Lockfile::verify_first_for_invention(&missing, &packs, &invention).unwrap();
}
