//! One test per way a frozen session could load package bytes its governing
//! lockfile never verified (see `AUDIT.md` of the frozen-lock fix): each
//! fails with resolution left to the loaders' own version choices.

use super::*;
use crate::invention::audio_assets::resolve_audio_assets;
use crate::modules::sample_loading::resolve_source;
use crate::pkg::audio_asset::with_packs_dir;
use crate::pkg::content::{ContentCatalog, ContentDetailQuery, ContentRef, ContentRoots};
use crate::pkg::{
    compute_integrity, resolve_package_asset, LockSource, LockedPackage, PackageAudioRef,
    LOCKFILE_NAME,
};
use serde_json::json;
use std::fs;

const KIT: &str = "fugue.test.kit";
const VOICE: &str = "fugue.test.voice";

fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// `fugue.test.kit` at `dir` (an installed version dir or a source
/// checkout): a sample pack whose `a.wav` holds `body`.
fn kit_at(dir: &Path, version: &str, body: &str) {
    write(
        &dir.join("fugue.pkg.json"),
        json!({"id": KIT, "version": version, "kind": "sample-pack", "license": "CC0-1.0",
            "authors": [{"name": "Test"}], "targets": ["external-agent"],
            "entry": {"samples": "samples.json"}})
        .to_string(),
    );
    write(&dir.join("a.wav"), body);
}

/// `fugue.test.voice@version`: a development playing `asset`, depending on
/// the kit.
fn voice(packs: &Path, version: &str, asset: &str) -> ContentRef {
    let dir = packs.join(VOICE).join(version);
    write(
        &dir.join("fugue.pkg.json"),
        json!({"id": VOICE, "version": version, "kind": "development", "license": "MIT",
            "authors": [{"name": "Test"}], "targets": ["external-agent"],
            "deps": [format!("{KIT}@^1.0.0")], "entry": {"development": "voice.json"}})
        .to_string(),
    );
    write(
        &dir.join("voice.json"),
        json!({"modules": [{"id": "s", "type": "sample_player", "config": {"asset": asset}}],
            "connections": [], "outputs": [{"name": "audio", "from": "s",
            "from_port": "audio_left"}]})
        .to_string(),
    );
    write(&dir.join("local.wav"), "voice");
    ContentRef::Package {
        package: VOICE.into(),
        version: version.into(),
    }
}

fn entry(dir: &Path, id: &str, version: &str) -> LockedPackage {
    LockedPackage {
        version: version.into(),
        kind: "sample-pack".into(),
        source: LockSource::Registry {
            id: id.into(),
            version: version.into(),
        },
        integrity: compute_integrity(dir).unwrap(),
        path: dir.to_path_buf(),
        dependencies: Vec::new(),
    }
}

/// Writes `dir/fugue.lock.json` locking each `(id, version)` as installed
/// under `packs`, and returns it as the governing lock.
fn lock(dir: &Path, packs: &Path, locked: &[(&str, &str)]) -> FrozenLock {
    let mut lockfile = Lockfile::new();
    for (id, version) in locked {
        let installed = packs.join(id).join(version);
        let installed = if installed.is_dir() {
            installed
        } else {
            packs.join(id)
        };
        lockfile.upsert(*id, entry(&installed, id, version));
    }
    let path = dir.join(LOCKFILE_NAME);
    write(&path, lockfile.to_bytes().unwrap());
    FrozenLock::find(&[path], packs).unwrap().unwrap()
}

/// The kit locked at 1.0.0, with a tampered, unlocked 1.1.0 installed
/// beside it: every unpinned resolution of `@1.0.0` (`^1.0.0`) picks 1.1.0.
fn kit_with_newer(dir: &Path) -> (PathBuf, FrozenLock) {
    let packs = dir.join("packs");
    kit_at(&packs.join(KIT).join("1.0.0"), "1.0.0", "locked");
    kit_at(&packs.join(KIT).join("1.1.0"), "1.1.0", "tampered newer");
    let lock = lock(dir, &packs, &[(KIT, "1.0.0")]);
    (packs, lock)
}

fn kit_ref(requirement: &str) -> PackageAudioRef {
    PackageAudioRef::parse(&format!("{KIT}@{requirement}:a.wav")).unwrap()
}

fn sampler_document(asset: &str, source: &Path) -> Invention {
    let mut invention: Invention = serde_json::from_value(json!({
        "modules": [{"id": "s", "type": "sample_player", "config": {"asset": asset}}],
        "connections": []
    }))
    .unwrap();
    invention.source_path = Some(source.to_path_buf());
    invention
}

fn asset_of(invention: &Invention) -> String {
    invention.modules[0].config["asset"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn control_writes_load_the_locked_version_not_a_newer_one() {
    // The cited Codex finding: an aliased `set_asset` resolved with no pin.
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = kit_with_newer(tmp.path());
    with_packs_dir(&packs, || {
        with_governing(lock, || {
            let path = resolve_source(&format!("{KIT}@1.0.0:a.wav")).unwrap();
            assert!(Path::new(&path).ends_with("1.0.0/a.wav"), "{path}");
        });
    });
}

#[test]
fn document_loads_pin_the_governing_lock_not_the_one_beside_the_invention() {
    // The daemon's governing lockfile is in its working dir or `~/.fugue`,
    // never beside the invention, which here has none.
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = kit_with_newer(tmp.path());
    let mut invention = sampler_document(
        &format!("{KIT}@1.0.0:a.wav"),
        &tmp.path().join("piece/main.json"),
    );
    fs::create_dir_all(tmp.path().join("piece")).unwrap();
    with_packs_dir(&packs, || {
        with_governing(lock, || {
            resolve_audio_assets(&mut invention).unwrap();
        });
    });
    assert!(Path::new(&asset_of(&invention)).ends_with("1.0.0/a.wav"));
}

#[test]
fn a_ref_the_locked_version_does_not_satisfy_is_refused_and_the_lock_kept() {
    // Unpinned, the loader would fall back to 1.1.0 and rewrite the lock.
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = kit_with_newer(tmp.path());
    let lock_path = lock.path().to_path_buf();
    let before = fs::read(&lock_path).unwrap();
    let mut invention =
        sampler_document(&format!("{KIT}@^1.1:a.wav"), &tmp.path().join("main.json"));
    with_packs_dir(&packs, || {
        with_governing(lock, || {
            let err = resolve_audio_assets(&mut invention)
                .unwrap_err()
                .to_string();
            assert!(err.contains("does not satisfy ^1.1"), "{err}");
            let err = resolve_source(&format!("{KIT}@^1.1:a.wav")).unwrap_err();
            assert!(err.contains("locks fugue.test.kit@1.0.0"), "{err}");
        });
    });
    assert_eq!(fs::read(&lock_path).unwrap(), before);
}

#[test]
fn a_tampered_locked_version_is_refused_where_it_resolves() {
    // A locked package the up-front check never hashed (a control write
    // reaching a package the document does not use).
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = kit_with_newer(tmp.path());
    write(&packs.join(KIT).join("1.0.0/a.wav"), "corrupted");
    with_governing(lock, || {
        let err = resolve_package_asset(&kit_ref("1.0.0"), &packs, None).unwrap_err();
        assert!(
            err.contains("fugue.test.kit@1.0.0 integrity mismatch"),
            "{err}"
        );
    });
}

#[test]
fn a_tampered_source_checkout_is_refused_where_it_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = tmp.path().join("packs");
    kit_at(&packs.join(KIT), "1.0.0", "checkout");
    let lock = lock(tmp.path(), &packs, &[(KIT, "1.0.0")]);
    write(&packs.join(KIT).join("a.wav"), "edited after locking");
    with_governing(lock, || {
        let err = resolve_package_asset(&kit_ref("1.0.0"), &packs, None).unwrap_err();
        assert!(err.contains("integrity mismatch"), "{err}");
    });
}

#[test]
fn development_refs_load_only_the_locked_version() {
    let tmp = tempfile::tempdir().unwrap();
    let (packs, _) = kit_with_newer(tmp.path());
    let locked = voice(&packs, "1.0.0", "local.wav");
    let newer = voice(&packs, "1.1.0", "local.wav");
    let roots = ContentRoots {
        packages: packs.clone(),
        workspace: tmp.path().join("inventions"),
    };
    let lock = lock(tmp.path(), &packs, &[(VOICE, "1.0.0")]);
    with_governing(lock, || {
        let err = roots.load_development(&newer).unwrap_err();
        assert!(
            err.message.contains("locks fugue.test.voice@1.0.0"),
            "{err}"
        );
        roots.load_development(&locked).unwrap();
        // Browsing the catalog is not loading: every version stays visible.
        ContentCatalog::new(roots.clone(), "session")
            .detail(&ContentDetailQuery {
                schema_version: 1,
                reference: newer.clone(),
            })
            .unwrap();
    });
}

#[test]
fn assets_inside_package_developments_pin_the_governing_lock() {
    // Unpinned, they follow a lockfile inside the package or its receipt.
    let tmp = tempfile::tempdir().unwrap();
    let (packs, _) = kit_with_newer(tmp.path());
    let reference = voice(&packs, "1.0.0", &format!("{KIT}@^1:a.wav"));
    let lock = lock(tmp.path(), &packs, &[(KIT, "1.0.0"), (VOICE, "1.0.0")]);
    let roots = ContentRoots {
        packages: packs,
        workspace: tmp.path().join("inventions"),
    };
    with_governing(lock, || {
        let definition = roots.load_development(&reference).unwrap();
        assert!(Path::new(&asset_of(&definition)).ends_with("1.0.0/a.wav"));
    });
}

#[test]
fn a_package_the_lockfile_does_not_lock_is_not_governed() {
    let tmp = tempfile::tempdir().unwrap();
    let (packs, _) = kit_with_newer(tmp.path());
    let lock = lock(tmp.path(), &packs, &[]);
    with_governing(lock, || {
        let resolved = resolve_package_asset(&kit_ref("1.0.0"), &packs, None).unwrap();
        assert_eq!(resolved.version.to_string(), "1.1.0");
    });
}
