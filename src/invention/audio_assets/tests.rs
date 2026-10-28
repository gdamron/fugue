use super::*;
use crate::pkg::audio_asset::with_packs_dir;
use crate::pkg::{Lockfile, LOCKFILE_NAME};
use std::fs;
use std::path::Path;

fn install_pack(packs: &Path, id: &str, version: &str, files: &[&str]) {
    let dir = packs.join(id).join(version);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("fugue.pkg.json"),
        format!(
            r#"{{"id":"{id}","version":"{version}","kind":"sample-pack","license":"CC0-1.0",
                "authors":[{{"name":"Test"}}],"entry":{{"samples":"samples.json"}}}}"#
        ),
    )
    .unwrap();
    for file in files {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"audio").unwrap();
    }
}

fn invention_with_asset(dir: &Path, asset: serde_json::Value) -> Invention {
    let json = serde_json::json!({
        "modules": [
            { "id": "kick", "type": "sample_player", "config": { "asset": asset } }
        ],
        "connections": []
    });
    let mut invention = Invention::from_json(&json.to_string()).unwrap();
    invention.source_path = Some(dir.join("groove.json"));
    invention
}

fn resolved_asset(invention: &Invention) -> &str {
    invention.modules[0].config["asset"].as_str().unwrap()
}

#[test]
fn local_object_resolves_relative_to_invention_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mut invention = invention_with_asset(
        tmp.path(),
        serde_json::json!({ "path": "./loops/melody.wav" }),
    );
    resolve_audio_assets(&mut invention, &Declarations::of(&Default::default(), None)).unwrap();
    assert_eq!(
        resolved_asset(&invention),
        tmp.path().join("./loops/melody.wav").to_string_lossy()
    );
    // No package refs: no lockfile appears.
    assert!(!tmp.path().join(LOCKFILE_NAME).exists());
}

#[test]
fn absolute_and_url_paths_pass_through() {
    let tmp = tempfile::tempdir().unwrap();
    let mut invention = invention_with_asset(tmp.path(), serde_json::json!("/abs/kick.wav"));
    resolve_audio_assets(&mut invention, &Declarations::of(&Default::default(), None)).unwrap();
    assert_eq!(resolved_asset(&invention), "/abs/kick.wav");

    let mut invention = invention_with_asset(
        tmp.path(),
        serde_json::json!("https://example.com/kick.wav"),
    );
    resolve_audio_assets(&mut invention, &Declarations::of(&Default::default(), None)).unwrap();
    assert_eq!(resolved_asset(&invention), "https://example.com/kick.wav");
}

#[test]
fn package_ref_resolves_and_records_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = tmp.path().join("packs");
    let inv_dir = tmp.path().join("song");
    fs::create_dir_all(&inv_dir).unwrap();
    install_pack(&packs, "fugue.drums.808", "1.2.0", &["kick/long.wav"]);
    with_packs_dir(&packs, || {
        let mut invention = invention_with_asset(
            &inv_dir,
            serde_json::json!("fugue.drums.808@1.2.0:kick/long.wav"),
        );
        resolve_audio_assets(&mut invention, &Declarations::of(&Default::default(), None)).unwrap();
        let resolved = resolved_asset(&invention).to_string();
        // Built with the resolver's own joins so separators match the
        // platform (the file path segment stays `/`-separated as
        // authored, appended whole).
        assert_eq!(
            resolved,
            packs
                .join("fugue.drums.808")
                .join("1.2.0")
                .join("kick/long.wav")
                .to_string_lossy()
        );

        let lock = Lockfile::read(&inv_dir.join(LOCKFILE_NAME)).unwrap();
        let entry = &lock.packages["fugue.drums.808"];
        assert_eq!(entry.version, "1.2.0");
        assert_eq!(entry.kind, "sample-pack");
        assert!(entry.integrity.starts_with("sha256:"));
        assert_eq!(
            entry.integrity,
            crate::pkg::compute_integrity(&packs.join("fugue.drums.808/1.2.0")).unwrap()
        );

        // Re-resolving the resolved document is a no-op (reload path).
        let before = fs::read(inv_dir.join(LOCKFILE_NAME)).unwrap();
        resolve_audio_assets(&mut invention, &Declarations::of(&Default::default(), None)).unwrap();
        assert_eq!(resolved_asset(&invention), resolved);
        assert_eq!(fs::read(inv_dir.join(LOCKFILE_NAME)).unwrap(), before);
    });
}

#[test]
fn locked_version_pins_and_keeps_recorded_integrity() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = tmp.path().join("packs");
    let inv_dir = tmp.path().join("song");
    fs::create_dir_all(&inv_dir).unwrap();
    install_pack(&packs, "fugue.drums.808", "1.2.0", &["kick.wav"]);
    install_pack(&packs, "fugue.drums.808", "1.4.0", &["kick.wav"]);
    with_packs_dir(&packs, || {
        // Pre-lock 1.2.0 with a sentinel integrity: the pin must win over
        // the newer install and the recorded hash must not be recomputed.
        let mut lock = Lockfile::new();
        lock.upsert(
            "fugue.drums.808",
            crate::pkg::LockedPackage {
                version: "1.2.0".into(),
                kind: "sample-pack".into(),
                source: crate::pkg::LockSource::Registry {
                    id: "fugue.drums.808".into(),
                    version: "1.2.0".into(),
                },
                integrity: "sha256:sentinel".into(),
                path: packs.join("fugue.drums.808/1.2.0"),
                dependencies: Vec::new(),
            },
        );
        fs::write(inv_dir.join(LOCKFILE_NAME), lock.to_bytes().unwrap()).unwrap();

        let mut invention =
            invention_with_asset(&inv_dir, serde_json::json!("fugue.drums.808@^1.2:kick.wav"));
        resolve_audio_assets(&mut invention, &Declarations::of(&Default::default(), None)).unwrap();
        assert!(resolved_asset(&invention).contains("1.2.0"));

        let lock = Lockfile::read(&inv_dir.join(LOCKFILE_NAME)).unwrap();
        assert_eq!(
            lock.packages["fugue.drums.808"].integrity,
            "sha256:sentinel"
        );
    });
}

#[test]
fn missing_package_names_module_in_error() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = tmp.path().join("packs");
    fs::create_dir_all(&packs).unwrap();
    with_packs_dir(&packs, || {
        let mut invention = invention_with_asset(
            tmp.path(),
            serde_json::json!("fugue.drums.808@1.2.0:kick.wav"),
        );
        let err =
            resolve_audio_assets(&mut invention, &Declarations::of(&Default::default(), None))
                .unwrap_err()
                .to_string();
        assert!(err.contains("module 'kick'"), "{err}");
        assert!(err.contains("no installed version"), "{err}");
    });
}

#[test]
fn invalid_asset_shape_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let mut invention = invention_with_asset(tmp.path(), serde_json::json!(42));
    let err = resolve_audio_assets(&mut invention, &Declarations::of(&Default::default(), None))
        .unwrap_err()
        .to_string();
    assert!(err.contains("invalid asset reference"), "{err}");
}

#[test]
fn sample_slot_refs_resolve_and_record_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = tmp.path().join("packs");
    let inv_dir = tmp.path().join("song");
    fs::create_dir_all(&inv_dir).unwrap();
    install_pack(&packs, "fugue.drums.808", "1.2.0", &["kick.wav"]);
    with_packs_dir(&packs, || {
        // Multi-sample configs (sample_kit) nest refs in a `samples`
        // array; both slots resolve and the package ref is locked.
        let json = serde_json::json!({
            "modules": [
                { "id": "kit", "type": "sample_kit", "config": { "samples": [
                    { "key": 36, "asset": "fugue.drums.808@1.2.0:kick.wav" },
                    { "key": 38, "asset": { "path": "./local/snare.wav" } }
                ] } }
            ],
            "connections": []
        });
        let mut invention = Invention::from_json(&json.to_string()).unwrap();
        invention.source_path = Some(inv_dir.join("groove.json"));
        resolve_audio_assets(&mut invention, &Declarations::of(&Default::default(), None)).unwrap();

        let samples = invention.modules[0].config["samples"].as_array().unwrap();
        assert_eq!(
            samples[0]["asset"].as_str().unwrap(),
            packs
                .join("fugue.drums.808")
                .join("1.2.0")
                .join("kick.wav")
                .to_string_lossy()
        );
        assert_eq!(
            samples[1]["asset"].as_str().unwrap(),
            inv_dir.join("./local/snare.wav").to_string_lossy()
        );

        let lock = Lockfile::read(&inv_dir.join(LOCKFILE_NAME)).unwrap();
        assert_eq!(lock.packages["fugue.drums.808"].version, "1.2.0");
    });
}

#[test]
fn zone_refs_resolve_and_record_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = tmp.path().join("packs");
    let inv_dir = tmp.path().join("song");
    fs::create_dir_all(&inv_dir).unwrap();
    install_pack(&packs, "fugue.keys.grand", "1.0.0", &["c4.wav"]);
    with_packs_dir(&packs, || {
        // Keymap configs (sample_instrument) nest refs in a `zones`
        // array; both zones resolve and the package ref is locked.
        let json = serde_json::json!({
            "modules": [
                { "id": "piano", "type": "sample_instrument", "config": { "zones": [
                    { "root_note": 60, "asset": "fugue.keys.grand@1.0.0:c4.wav" },
                    { "root_note": 72, "asset": { "path": "./local/c5.wav" } }
                ] } }
            ],
            "connections": []
        });
        let mut invention = Invention::from_json(&json.to_string()).unwrap();
        invention.source_path = Some(inv_dir.join("nocturne.json"));
        resolve_audio_assets(&mut invention, &Declarations::of(&Default::default(), None)).unwrap();

        let zones = invention.modules[0].config["zones"].as_array().unwrap();
        assert_eq!(
            zones[0]["asset"].as_str().unwrap(),
            packs
                .join("fugue.keys.grand")
                .join("1.0.0")
                .join("c4.wav")
                .to_string_lossy()
        );
        assert_eq!(
            zones[1]["asset"].as_str().unwrap(),
            inv_dir.join("./local/c5.wav").to_string_lossy()
        );

        let lock = Lockfile::read(&inv_dir.join(LOCKFILE_NAME)).unwrap();
        assert_eq!(lock.packages["fugue.keys.grand"].version, "1.0.0");
    });
}
