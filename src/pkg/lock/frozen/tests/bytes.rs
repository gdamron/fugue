//! A governed load consumes only the bytes the lockfile verified, and the
//! session's lock changes only when a candidate commits (the second Codex
//! round on the frozen-lock fix).

use super::*;
use crate::invention::manual_backend::Settled;
use crate::invention::InventionBuilder;
use crate::modules::sample_loading::{load_cached_sample, load_sample_source};

/// A WAV of eight frames, each `value`.
pub(super) fn wav(path: &Path, value: f32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for _ in 0..8 {
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();
}

/// [`kit_with_newer`], with decodable samples: 0.25 locked, 0.75 newer.
pub(super) fn playable_kit(dir: &Path) -> (PathBuf, FrozenLock) {
    let packs = dir.join("packs");
    kit_at(&packs.join(KIT).join("1.0.0"), "1.0.0", "");
    kit_at(&packs.join(KIT).join("1.1.0"), "1.1.0", "");
    wav(&packs.join(KIT).join("1.0.0/a.wav"), 0.25);
    wav(&packs.join(KIT).join("1.1.0/a.wav"), 0.75);
    let lock = lock(dir, &packs, &[(KIT, "1.0.0")]);
    (packs, lock)
}

#[test]
fn a_file_edited_after_verification_is_refused_where_it_resolves_and_where_it_is_read() {
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = playable_kit(tmp.path());
    with_packs_dir(&packs, || {
        with_governing(lock, || {
            let source = format!("{KIT}@1.0.0:a.wav");
            let (path, _) = load_sample_source(&source, 48_000).unwrap();
            wav(Path::new(&path), 0.5);
            // Read again at the path it resolved to, as a reload would.
            let err = load_cached_sample(&path, 48_000).err().unwrap();
            assert!(err.contains("changed since the lockfile verified"), "{err}");
            let err = resolve_source(&source).unwrap_err();
            assert!(err.contains("changed since the lockfile verified"), "{err}");
        });
    });
}

#[test]
fn a_governed_load_decodes_the_verified_bytes_not_an_earlier_decode_of_others() {
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = playable_kit(tmp.path());
    let path = packs.join(KIT).join("1.0.0/a.wav");
    with_packs_dir(&packs, || {
        // Ungoverned, other bytes at the locked path are decoded and cached.
        wav(&path, 0.5);
        let other = load_cached_sample(path.to_str().unwrap(), 48_000).unwrap();
        assert_eq!(other.left[0], 0.5);
        wav(&path, 0.25);
        with_governing(lock, || {
            let (_, sample) = load_sample_source(&format!("{KIT}@1.0.0:a.wav"), 48_000).unwrap();
            assert_eq!(sample.left[0], 0.25);
        });
    });
}

fn kit_file(path: &str) -> PackageAudioRef {
    PackageAudioRef::parse(&format!("{KIT}@=1.0.0:{path}")).unwrap()
}

#[test]
fn files_the_integrity_skips_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = kit_with_newer(tmp.path());
    let installed = packs.join(KIT).join("1.0.0");
    write(&installed.join(".git/a.wav"), "never hashed");
    #[cfg(unix)]
    std::os::unix::fs::symlink(installed.join("a.wav"), installed.join("link.wav")).unwrap();
    with_governing(lock, || {
        let err = resolve_package_asset(&kit_file(".git/a.wav"), &packs, None).unwrap_err();
        assert!(err.contains("not among the files"), "{err}");
        #[cfg(unix)]
        {
            let link = kit_file("link.wav");
            let err = resolve_package_asset(&link, &packs, None).unwrap_err();
            assert!(err.contains("not among the files"), "{err}");
        }
    });
}

#[test]
fn a_candidate_governs_only_the_thread_building_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = kit_with_newer(tmp.path());
    let version = |packs: &Path| {
        resolve_package_asset(&kit_ref("^1"), packs, None)
            .unwrap()
            .version
            .to_string()
    };
    with_governing(lock, || {
        govern_here(None, || {
            assert_eq!(version(&packs), "1.1.0", "the candidate is ungoverned");
            // The session's other threads, its scripts, keep its lock.
            let session = std::thread::scope(|scope| scope.spawn(|| version(&packs)).join());
            assert_eq!(session.unwrap(), "1.0.0");
        });
        assert_eq!(version(&packs), "1.0.0");
    });
}

pub(super) fn sampler(asset: &str) -> Invention {
    serde_json::from_value(json!({
        "modules": [{"id": "s", "type": "sample_player", "config": {"asset": asset}},
            {"id": "dac", "type": "dac"}],
        "connections": [{"from": "s", "from_port": "audio_left", "to": "dac", "to_port": "audio"}]
    }))
    .unwrap()
}

#[test]
fn a_reload_adopts_its_lock_as_it_commits_and_a_refused_one_keeps_the_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = playable_kit(tmp.path());
    let path = lock.path().to_path_buf();
    let locked = &format!("{KIT}@1.0.0:a.wav");
    let is_session = |expected: Option<&Path>| {
        assert_eq!(
            governing().map(|lock| lock.path().to_path_buf()).as_deref(),
            expected
        );
    };
    with_packs_dir(&packs, || {
        with_governing(lock, || {
            let session = governing();
            let (runtime, _) = InventionBuilder::new(48_000)
                .build(sampler(locked))
                .unwrap();
            let mut running = Settled::start(runtime);

            let newer = sampler(&format!("{KIT}@^1.1:a.wav"));
            let err = running
                .reload_governed(newer.clone(), session.clone())
                .unwrap_err();
            assert!(err.to_string().contains("does not satisfy"), "{err}");
            is_session(Some(&path));

            running.reload_governed(newer, None).unwrap();
            is_session(None);

            running.reload_governed(sampler(locked), session).unwrap();
            is_session(Some(&path));
        });
    });
}
