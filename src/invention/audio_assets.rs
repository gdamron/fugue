//! Load-time resolution of module `asset` config references (FUG-130).
//!
//! The invention format lets any module config reference an audio asset under
//! a top-level `asset` key, authored either as a package ref string or as a
//! local path object (see [`crate::pkg::AudioAssetRef`]). Before modules are
//! built, [`resolve_audio_assets`] rewrites each reference to the concrete
//! file path it resolves to, in this order:
//!
//! 1. **package cache** — refs like `fugue.drums.808@1.2.0:kick/long.wav`
//!    resolve into the installed package cache,
//! 2. **relative to the invention file** — relative paths join the loaded
//!    document's directory,
//! 3. **absolute path** — used as authored.
//!
//! Every package ref resolution is recorded in `fugue.lock.json` beside the
//! invention file: the resolved version and an integrity hash over the
//! installed package contents, so `--frozen` loads can verify the exact audio
//! that was resolved. An already-locked version keeps winning while it still
//! satisfies the ref and is installed, and its recorded integrity is left
//! untouched (verification is the frozen load's job, not the resolver's).
//!
//! The authored document (retained for saves) is cloned before this pass
//! runs, so saved inventions keep their refs instead of machine paths.

use super::format::Invention;

/// Rewrites every module-config `asset` reference in `invention` to its
/// resolved file path, recording package resolutions in the lockfile beside
/// the invention file. Idempotent: resolved path strings pass through
/// unchanged, so re-resolving an already-resolved document (as reload does)
/// is a no-op.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_audio_assets(
    invention: &mut Invention,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::pkg::AudioAssetRef;

    let base_dir = invention
        .source_path
        .as_deref()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf);
    let mut lock = LazyLockfile::new(base_dir.as_deref());

    for module in &mut invention.modules {
        let Some(config) = module.config.as_object_mut() else {
            continue;
        };
        let Some(value) = config.get("asset") else {
            continue;
        };
        let asset: AudioAssetRef = serde_json::from_value(value.clone()).map_err(|err| {
            format!(
                "module '{}': invalid asset reference {}: {}",
                module.id, value, err
            )
        })?;
        let resolved = resolve_ref(&asset, base_dir.as_deref(), &mut lock)
            .map_err(|err| format!("module '{}': {}", module.id, err))?;
        config.insert("asset".to_string(), serde_json::Value::String(resolved));
    }

    lock.write_if_dirty()
}

/// On wasm there is no filesystem: package refs are left for the module to
/// reject at load time, and local path objects flatten to their path string.
#[cfg(target_arch = "wasm32")]
pub(crate) fn resolve_audio_assets(
    invention: &mut Invention,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::pkg::AudioAssetRef;

    for module in &mut invention.modules {
        let Some(config) = module.config.as_object_mut() else {
            continue;
        };
        let Some(value) = config.get("asset") else {
            continue;
        };
        let asset: AudioAssetRef = serde_json::from_value(value.clone()).map_err(|err| {
            format!(
                "module '{}': invalid asset reference {}: {}",
                module.id, value, err
            )
        })?;
        if let AudioAssetRef::Local { path } = asset {
            config.insert("asset".to_string(), serde_json::Value::String(path));
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
use non_wasm::{resolve_ref, LazyLockfile};

#[cfg(not(target_arch = "wasm32"))]
mod non_wasm {
    use std::path::{Path, PathBuf};

    use crate::pkg::{
        compute_integrity, default_packages_dir, resolve_package_asset, AudioAssetRef, LockSource,
        LockedPackage, Lockfile, PackageAudioRef, ResolvedPackageAsset, LOCKFILE_NAME,
    };

    /// Resolves one authored reference to a load-ready path string.
    pub(super) fn resolve_ref(
        asset: &AudioAssetRef,
        base_dir: Option<&Path>,
        lock: &mut LazyLockfile,
    ) -> Result<String, String> {
        match asset {
            AudioAssetRef::Text(text) => match PackageAudioRef::parse(text) {
                Some(reference) => {
                    let packages_dir = default_packages_dir()?;
                    let resolved = resolve_package_asset(
                        &reference,
                        &packages_dir,
                        lock.version_of(&reference.id)?,
                    )?;
                    let path = resolved.file.to_string_lossy().into_owned();
                    lock.record(resolved)?;
                    Ok(path)
                }
                None => Ok(resolve_local(text, base_dir)),
            },
            AudioAssetRef::Local { path } => Ok(resolve_local(path, base_dir)),
        }
    }

    /// Local path resolution: URLs and absolute paths pass through; relative
    /// paths join the invention file's directory when one is known.
    fn resolve_local(path: &str, base_dir: Option<&Path>) -> String {
        if path.starts_with("https://") || path.starts_with("http://") {
            return path.to_string();
        }
        let candidate = Path::new(path);
        if candidate.is_absolute() {
            return path.to_string();
        }
        match base_dir {
            Some(base) => base.join(candidate).to_string_lossy().into_owned(),
            None => path.to_string(),
        }
    }

    /// The lockfile beside the invention file, read on first package ref and
    /// written back only when a resolution changed it. Inventions loaded
    /// without a source path resolve normally but record nothing (there is
    /// nowhere canonical to put a lockfile).
    pub(super) struct LazyLockfile {
        path: Option<PathBuf>,
        lockfile: Option<Lockfile>,
        dirty: bool,
    }

    impl LazyLockfile {
        pub(super) fn new(base_dir: Option<&Path>) -> Self {
            Self {
                path: base_dir.map(|dir| dir.join(LOCKFILE_NAME)),
                lockfile: None,
                dirty: false,
            }
        }

        fn lockfile(&mut self) -> Result<Option<&mut Lockfile>, String> {
            let Some(path) = &self.path else {
                return Ok(None);
            };
            if self.lockfile.is_none() {
                self.lockfile = Some(
                    Lockfile::read_or_new(path)
                        .map_err(|err| format!("failed to read {}: {}", path.display(), err))?,
                );
            }
            Ok(self.lockfile.as_mut())
        }

        /// The locked version pin for `id`, if any.
        pub(super) fn version_of(&mut self, id: &str) -> Result<Option<&str>, String> {
            // Borrow-checker friendly: load first, then read through self.
            self.lockfile()?;
            Ok(self
                .lockfile
                .as_ref()
                .and_then(|lock| lock.packages.get(id))
                .map(|package| package.version.as_str()))
        }

        /// Records a resolution. An entry already locked to the same version
        /// is left untouched (its integrity hash included); anything else is
        /// (re)written with a freshly computed integrity hash.
        pub(super) fn record(&mut self, resolved: ResolvedPackageAsset) -> Result<(), String> {
            let kind = installed_kind(&resolved.install_dir);
            let Some(lockfile) = self.lockfile()? else {
                return Ok(());
            };
            let version = resolved.version.to_string();
            if lockfile
                .packages
                .get(&resolved.id)
                .is_some_and(|package| package.version == version)
            {
                return Ok(());
            }
            let integrity = compute_integrity(&resolved.install_dir).map_err(|err| {
                format!(
                    "failed to hash installed contents of {}@{}: {}",
                    resolved.id, version, err
                )
            })?;
            lockfile.upsert(
                resolved.id.clone(),
                LockedPackage {
                    source: LockSource::Registry {
                        id: resolved.id,
                        version: version.clone(),
                    },
                    version,
                    kind,
                    integrity,
                    path: resolved.install_dir,
                    dependencies: Vec::new(),
                },
            );
            self.dirty = true;
            Ok(())
        }

        pub(super) fn write_if_dirty(self) -> Result<(), Box<dyn std::error::Error>> {
            if !self.dirty {
                return Ok(());
            }
            let (Some(path), Some(lockfile)) = (self.path, self.lockfile) else {
                return Ok(());
            };
            std::fs::write(&path, lockfile.to_bytes()?)
                .map_err(|err| format!("failed to write {}: {}", path.display(), err))?;
            Ok(())
        }
    }

    /// The installed package's declared kind, read leniently from its
    /// `fugue.pkg.json` (asset refs point into sample packs by design, so
    /// that is the fallback when the manifest is missing or malformed).
    fn installed_kind(install_dir: &Path) -> String {
        std::fs::read(install_dir.join("fugue.pkg.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|manifest| manifest.get("kind")?.as_str().map(str::to_string))
            .unwrap_or_else(|| "sample-pack".to_string())
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
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
        resolve_audio_assets(&mut invention).unwrap();
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
        resolve_audio_assets(&mut invention).unwrap();
        assert_eq!(resolved_asset(&invention), "/abs/kick.wav");

        let mut invention = invention_with_asset(
            tmp.path(),
            serde_json::json!("https://example.com/kick.wav"),
        );
        resolve_audio_assets(&mut invention).unwrap();
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
            resolve_audio_assets(&mut invention).unwrap();
            let resolved = resolved_asset(&invention).to_string();
            assert_eq!(
                resolved,
                packs
                    .join("fugue.drums.808/1.2.0/kick/long.wav")
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
            resolve_audio_assets(&mut invention).unwrap();
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
            resolve_audio_assets(&mut invention).unwrap();
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
            let err = resolve_audio_assets(&mut invention)
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
        let err = resolve_audio_assets(&mut invention)
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid asset reference"), "{err}");
    }
}
