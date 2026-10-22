//! A file is governed by where it lies, not by how its path is spelled or
//! which loader reads it, and a reload under another policy keeps nothing
//! loaded under the session's (the third Codex round on the frozen-lock fix).

use super::bytes::{playable_kit, sampler, wav};
use super::*;
use crate::invention::manual_backend::Settled;
use crate::invention::InventionBuilder;
use crate::modules::sample_loading::{load_cached_sample, load_sample_source};

#[test]
fn a_file_named_through_another_spelling_of_the_packages_dir_is_governed() {
    // A development's samples are canonicalized as they resolve, so a load
    // can name a governed file through a spelling the lock was not given.
    let tmp = tempfile::tempdir().unwrap();
    let (packs, _) = playable_kit(tmp.path());
    let link = tmp.path().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&packs, &link).unwrap();
    #[cfg(not(unix))]
    return;
    let lock = lock(tmp.path(), &link, &[(KIT, "1.0.0")]);
    let canonical = fs::canonicalize(&packs)
        .unwrap()
        .join(KIT)
        .join("1.0.0/a.wav");
    with_packs_dir(&link, || {
        with_governing(lock, || {
            load_sample_source(&format!("{KIT}@1.0.0:a.wav"), 48_000).unwrap();
            wav(&canonical, 0.5);
            let err = load_cached_sample(canonical.to_str().unwrap(), 48_000)
                .err()
                .unwrap();
            assert!(err.contains("changed since the lockfile verified"), "{err}");
        });
    });
}

#[test]
fn a_file_of_another_version_of_a_locked_package_is_refused_however_it_is_named() {
    // A restored session replays the absolute path an earlier lock resolved.
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = playable_kit(tmp.path());
    let newer = packs.join(KIT).join("1.1.0/a.wav");
    with_governing(lock, || {
        let err = load_cached_sample(newer.to_str().unwrap(), 48_000)
            .err()
            .unwrap();
        assert!(err.contains("not in fugue.test.kit@1.0.0"), "{err}");
    });
}

fn slicer(asset: &str) -> Invention {
    serde_json::from_value(json!({
        "modules": [{"id": "s", "type": "sample_slicer", "config": {"asset": asset}}],
        "connections": []
    }))
    .unwrap()
}

fn sidecar(dir: &Path, cut: u32) {
    write(
        &dir.join("samples.json"),
        json!({"license": "CC0-1.0", "sample_rate": [48000], "files": [{"path": "a.wav",
            "slices": [{"start_frames": 0, "end_frames": cut},
                {"start_frames": cut, "end_frames": 8}]}]})
        .to_string(),
    );
}

#[test]
fn a_sidecar_edited_after_verification_is_refused_as_a_later_load_reads_it() {
    let tmp = tempfile::tempdir().unwrap();
    let packs = tmp.path().join("packs");
    let installed = packs.join(KIT).join("1.0.0");
    kit_at(&installed, "1.0.0", "");
    wav(&installed.join("a.wav"), 0.25);
    sidecar(&installed, 4);
    let lock = lock(tmp.path(), &packs, &[(KIT, "1.0.0")]);
    let build = || InventionBuilder::new(48_000).build(slicer(&format!("{KIT}@1.0.0:a.wav")));
    with_packs_dir(&packs, || {
        with_governing(lock, || {
            build().unwrap();
            sidecar(&installed, 2);
            let err = build().err().unwrap().to_string();
            assert!(err.contains("changed since the lockfile verified"), "{err}");
        });
    });
}

#[test]
fn a_content_entry_edited_after_verification_is_refused_as_it_loads() {
    let tmp = tempfile::tempdir().unwrap();
    let (packs, _) = kit_with_newer(tmp.path());
    let locked = voice(&packs, "1.0.0", "local.wav");
    let roots = ContentRoots {
        packages: packs.clone(),
        workspace: tmp.path().join("inventions"),
    };
    let lock = lock(tmp.path(), &packs, &[(VOICE, "1.0.0")]);
    with_governing(lock, || {
        roots.load_development(&locked).unwrap();
        let entry = packs.join(VOICE).join("1.0.0/voice.json");
        let edited = fs::read_to_string(&entry)
            .unwrap()
            .replace("\"s\"", "\"t\"");
        write(&entry, edited);
        let err = roots.load_development(&locked).unwrap_err();
        assert!(err.message.contains("changed since the lockfile"), "{err}");
    });
}

#[test]
fn a_reload_under_another_policy_rebuilds_what_may_hold_package_content() {
    // An absolute path (or an alias inside a development) does not change
    // with the policy, so the diff alone would keep what it loaded.
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = playable_kit(tmp.path());
    let mut document = sampler(packs.join(KIT).join("1.0.0/a.wav").to_str().unwrap());
    document
        .modules
        .push(serde_json::from_value(json!({"id": "o", "type": "oscillator"})).unwrap());
    with_packs_dir(&packs, || {
        with_governing(lock, || {
            let lock = govern(None);
            let (runtime, _) = InventionBuilder::new(48_000)
                .build(document.clone())
                .unwrap();
            let mut running = Settled::start(runtime);

            let governed = running
                .reload_governed(document.clone(), lock.clone())
                .unwrap();
            assert_eq!(governed.swapped, ["s"], "{governed:?}");

            let same = running.reload_governed(document, lock).unwrap();
            assert!(same.swapped.is_empty(), "{same:?}");
        });
    });
}
