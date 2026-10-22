//! A file is governed by where it lies, not by how its path is spelled or
//! which loader reads it, a reload under another policy than its graph was
//! built under keeps nothing that graph loaded, and browsing is never
//! governed (the third and fourth Codex rounds on the frozen-lock fix).

use super::bytes::{playable_kit, sampler, wav};
use super::*;
use crate::invention::manual_backend::Settled;
use crate::invention::InventionBuilder;
use crate::modules::sample_loading::{load_cached_sample, load_sample_source};
use crate::rpc::{EditWhen, RequestState};

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

#[test]
fn a_file_named_through_a_parent_traversal_is_governed() {
    // `packs/../packs/<id>/..` names the packages dir as configured, but
    // where it lies is the filesystem's to say.
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = playable_kit(tmp.path());
    let traversed = packs.join("../packs").join(KIT).join("1.1.0/a.wav");
    with_governing(lock, || {
        let err = load_cached_sample(traversed.to_str().unwrap(), 48_000)
            .err()
            .unwrap();
        assert!(err.contains("not in fugue.test.kit@1.0.0"), "{err}");
    });
}

#[test]
fn a_file_named_through_another_case_of_a_locked_id_is_governed() {
    // On a case-insensitive volume (macOS's default), `FUGUE.test.kit` is
    // the locked package's dir.
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = playable_kit(tmp.path());
    let alias = packs.join(KIT.to_uppercase());
    if !alias.exists() {
        return; // A case-sensitive volume: another id, which is not locked.
    }
    with_governing(lock, || {
        let newer = alias.join("1.1.0/a.wav");
        let err = load_cached_sample(newer.to_str().unwrap(), 48_000)
            .err()
            .unwrap();
        assert!(err.contains("not in fugue.test.kit@1.0.0"), "{err}");
        wav(&packs.join(KIT).join("1.0.0/a.wav"), 0.5);
        let edited = alias.join("1.0.0/a.wav");
        let err = load_cached_sample(edited.to_str().unwrap(), 48_000)
            .err()
            .unwrap();
        assert!(err.contains("changed since the lockfile verified"), "{err}");
    });
}

#[test]
fn a_reload_compares_the_policy_its_graph_was_built_under_not_the_sessions() {
    // A timed reload under no lock lands, and its graph plays, before the
    // host makes its lock the session's: a reload under the session's lock
    // then changes the policy of what plays.
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = playable_kit(tmp.path());
    let mut document = sampler(packs.join(KIT).join("1.0.0/a.wav").to_str().unwrap());
    document
        .modules
        .push(serde_json::from_value(json!({"id": "l", "type": "lfo"})).unwrap());
    with_packs_dir(&packs, || {
        with_governing(lock, || {
            let session = governing();
            let (runtime, _) = InventionBuilder::new(48_000)
                .build(sampler(
                    packs.join(KIT).join("1.0.0/a.wav").to_str().unwrap(),
                ))
                .unwrap();
            let mut running = Settled::start(runtime);

            let at_once = EditWhen::AtSample { sample: 0 };
            let queued =
                govern_here(None, || running.reload_at(document.clone(), &at_once, None)).unwrap();
            running.pump.render(1);
            let landed = running.request_status(&queued.id).unwrap();
            assert_eq!(landed.state, RequestState::Adopted, "{landed:?}");

            let report = running.reload_governed(document, session).unwrap();
            assert_eq!(report.swapped, ["s"], "{report:?}");
        });
    });
}

#[test]
fn browsing_another_version_with_a_json_asset_is_not_governed() {
    let tmp = tempfile::tempdir().unwrap();
    let (packs, _) = kit_with_newer(tmp.path());
    let locked = voice(&packs, "1.0.0", "local.wav");
    let newer = voice(&packs, "1.1.0", "local.wav");
    let document = packs.join(VOICE).join("1.1.0/voice.json");
    let mut voice: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&document).unwrap()).unwrap();
    voice["assets"] = json!({"tuning": {"path": "tuning.json"}});
    write(&document, voice.to_string());
    write(&packs.join(VOICE).join("1.1.0/tuning.json"), "{}");
    let roots = ContentRoots {
        packages: packs.clone(),
        workspace: tmp.path().join("inventions"),
    };
    let lock = lock(tmp.path(), &packs, &[(VOICE, "1.0.0")]);
    with_governing(lock, || {
        roots.load_development(&locked).unwrap();
        ContentCatalog::new(roots.clone(), "session")
            .detail(&ContentDetailQuery {
                schema_version: 1,
                reference: newer,
            })
            .unwrap();
    });
}
