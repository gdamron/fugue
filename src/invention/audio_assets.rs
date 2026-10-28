//! Load-time resolution of module `asset` config references.
//!
//! The invention format lets any module config reference an audio asset under
//! a top-level `asset` key — or, for multi-sample modules like `sample_kit`
//! and `sample_instrument`, an `asset` key inside each entry of a top-level
//! `samples` or `zones` array — authored
//! either as a package ref string or as a local path object (see
//! [`crate::pkg::AudioAssetRef`]). Before modules are built,
//! [`resolve_audio_assets`] rewrites each reference to the concrete file path
//! it resolves to, in this order:
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

use std::borrow::Cow;

use super::format::Invention;
use crate::ControlValue;

/// Whether `key` is shaped as a control that names an audio asset: a
/// module's `asset`, or a sample's `asset.N`. A development instance's
/// control so named loads audio only as its development declares it (see
/// [`AudioControls`]).
pub(crate) fn is_asset_control(key: &str) -> bool {
    key == "asset"
        || key
            .strip_prefix("asset.")
            .is_some_and(|index| index.parse::<usize>().is_ok())
}

/// The value the runtime's stored module config keeps for a control write,
/// to a control that loads the audio it names when `asset`.
///
/// The stored config holds assets resolved, as [`resolve_audio_assets`]
/// left them at load, so a package ref written to an asset control is
/// stored as the file it resolves to: a reload of a saved document naming
/// the same ref then diffs equal and keeps the module. Any other write, and
/// a ref that does not resolve, is kept as written.
pub(crate) fn stored_asset_write(asset: bool, value: &ControlValue) -> Cow<'_, ControlValue> {
    match value {
        ControlValue::String(text)
            if asset && crate::pkg::PackageAudioRef::parse(text).is_some() =>
        {
            match crate::modules::sample_loading::resolve_source(text) {
                Ok(resolved) => Cow::Owned(ControlValue::String(resolved)),
                Err(_) => Cow::Borrowed(value),
            }
        }
        _ => Cow::Borrowed(value),
    }
}

mod developments;
mod live;

pub(crate) use developments::{audio_slots, AudioControls, Declarations};
pub(crate) use live::{
    is_relative_path, rebase_asset_write, resolve_asset_write, resolve_checked_asset_write,
};

/// Rewrites every module-config `asset` reference in `invention` to its
/// resolved file path, recording package resolutions in the lockfile beside
/// the invention file. Idempotent: resolved path strings pass through
/// unchanged, so re-resolving an already-resolved document (as reload does)
/// is a no-op.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_audio_assets(
    invention: &mut Invention,
    declarations: &Declarations<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::pkg::AudioAssetRef;

    let base_dir = invention
        .source_path
        .as_deref()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf);
    let mut lock = LazyLockfile::new(base_dir.as_deref());

    for module in &mut invention.modules {
        let module_id = module.id.clone();
        for value in audio_slots(module, declarations) {
            let asset: AudioAssetRef = serde_json::from_value(value.clone()).map_err(|err| {
                format!(
                    "module '{}': invalid asset reference {}: {}",
                    module_id, value, err
                )
            })?;
            let resolved = resolve_ref(&asset, base_dir.as_deref(), &mut lock)
                .map_err(|err| format!("module '{}': {}", module_id, err))?;
            *value = serde_json::Value::String(resolved);
        }
    }

    lock.write_if_dirty()
}

/// On wasm there is no filesystem: package refs are left for the module to
/// reject at load time, and local path objects flatten to their path string.
#[cfg(target_arch = "wasm32")]
pub(crate) fn resolve_audio_assets(
    invention: &mut Invention,
    declarations: &Declarations<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::pkg::AudioAssetRef;

    for module in &mut invention.modules {
        let module_id = module.id.clone();
        for value in audio_slots(module, declarations) {
            let asset: AudioAssetRef = serde_json::from_value(value.clone()).map_err(|err| {
                format!(
                    "module '{}': invalid asset reference {}: {}",
                    module_id, value, err
                )
            })?;
            if let AudioAssetRef::Local { path } = asset {
                *value = serde_json::Value::String(path);
            }
        }
    }
    Ok(())
}

/// The audio asset reference slots of one module config: the top-level
/// `asset` value, plus each `asset` value inside entries of a top-level
/// `samples` array (multi-sample modules like `sample_kit`) or `zones`
/// array (`sample_instrument`).
pub(crate) fn module_asset_values(
    config: &mut serde_json::Value,
) -> impl Iterator<Item = &mut serde_json::Value> {
    module_asset_slots(config).map(|(_, value)| value)
}

/// [`module_asset_values`], each with the top-level key it lies under.
pub(super) fn module_asset_slots(
    config: &mut serde_json::Value,
) -> impl Iterator<Item = (&'static str, &mut serde_json::Value)> {
    let (asset, samples, zones) = match config.as_object_mut() {
        Some(object) => {
            // Split borrows: `remove`-free simultaneous access needs raw
            // iteration, so walk the map once and keep the keys apart.
            let mut asset = None;
            let mut samples = None;
            let mut zones = None;
            for (key, value) in object.iter_mut() {
                match key.as_str() {
                    "asset" => asset = Some(value),
                    "samples" => samples = value.as_array_mut(),
                    "zones" => zones = value.as_array_mut(),
                    _ => {}
                }
            }
            (asset, samples, zones)
        }
        None => (None, None, None),
    };
    (asset.map(|value| ("asset", value)).into_iter())
        .chain(entries("samples", samples))
        .chain(entries("zones", zones))
}

/// The `asset` value of each entry of `list`, under `key`.
fn entries<'a>(
    key: &'static str,
    list: Option<&'a mut Vec<serde_json::Value>>,
) -> impl Iterator<Item = (&'static str, &'a mut serde_json::Value)> {
    (list.into_iter().flatten())
        .filter_map(|entry| entry.as_object_mut()?.get_mut("asset"))
        .map(move |value| (key, value))
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

    /// Local path resolution: URLs and rooted paths pass through; relative
    /// paths join the invention file's directory when one is known.
    pub(super) fn resolve_local(path: &str, base_dir: Option<&Path>) -> String {
        match base_dir.filter(|_| is_relative_local(path)) {
            Some(base) => base.join(path).to_string_lossy().into_owned(),
            None => path.to_string(),
        }
    }

    /// Whether a local path joins the invention file's directory: neither a
    /// URL nor rooted.
    pub(super) fn is_relative_local(path: &str) -> bool {
        // `has_root`, not `is_absolute`: on Windows a rooted path like
        // `/abs/kick.wav` is not "absolute" (no drive prefix), and `join`
        // would splice it onto the invention's drive instead of passing the
        // authored path through.
        !(path.starts_with("https://") || path.starts_with("http://") || Path::new(path).has_root())
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
mod tests;
