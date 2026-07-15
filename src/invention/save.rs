//! Saving a declarative invention document back to disk.
//!
//! The runtime retains the authored document alongside the built graph (see
//! [`crate::RuntimeState::document`]); this module writes that document to a
//! file so live mutations round-trip losslessly to the invention format.

use std::path::{Path, PathBuf};

use crate::Invention;

impl Invention {
    /// Writes the document to `path` as pretty JSON, atomically (temp file +
    /// rename) so a crash mid-write never leaves a half-written file behind.
    ///
    /// Relative development and asset paths are authored relative to the
    /// document's `source_path`; when saving to a different directory they
    /// are rebased so the saved file still resolves them: relative to the
    /// target directory when the referenced file sits under it, absolute
    /// otherwise. Documents without a `source_path` are written as-is.
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), Box<dyn std::error::Error>> {
        let path = path.as_ref();
        let rebased = self.rebased_for(path)?;
        let mut json = rebased.to_json()?;
        json.push('\n');

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".tmp");
        let tmp = path.with_file_name(tmp_name);
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Returns a copy whose relative development and asset paths are rebased
    /// from this document's `source_path` to `target`'s directory.
    fn rebased_for(&self, target: &Path) -> Result<Invention, Box<dyn std::error::Error>> {
        let mut document = self.clone();
        let Some(source_dir) = self.source_path.as_deref().and_then(Path::parent) else {
            return Ok(document);
        };
        let target_dir = match target.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => std::path::absolute(parent)?,
            _ => std::env::current_dir()?,
        };

        for spec in &mut document.developments {
            if let Some(dev_path) = spec.path.as_mut() {
                *dev_path = rebase(dev_path, source_dir, &target_dir)?;
            }
        }
        for asset in document.assets.values_mut() {
            asset.path = rebase(&asset.path, source_dir, &target_dir)?;
        }
        Ok(document)
    }
}

/// Rebases one authored path from `source_dir` to `target_dir`. Absolute
/// paths pass through; a relative path resolves against `source_dir` and
/// comes back relative to `target_dir` when it sits under it, absolute
/// otherwise.
fn rebase(
    authored: &str,
    source_dir: &Path,
    target_dir: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let authored_path = PathBuf::from(authored);
    if authored_path.is_absolute() {
        return Ok(authored.to_string());
    }
    let resolved = std::path::absolute(source_dir.join(&authored_path))?;
    let rebased = match resolved.strip_prefix(target_dir) {
        Ok(relative) => relative.to_path_buf(),
        Err(_) => resolved,
    };
    Ok(rebased.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use crate::Invention;
    use std::path::PathBuf;

    fn doc_with_refs(source: Option<&str>) -> Invention {
        let mut invention = Invention::from_json(
            r#"{
                "version": "1.0.0",
                "title": "refs",
                "developments": [{ "name": "voice", "path": "voice.json" }],
                "assets": { "score": { "path": "assets/score.json" } },
                "modules": [{ "id": "dac", "type": "dac" }],
                "connections": []
            }"#,
        )
        .unwrap();
        invention.source_path = source.map(PathBuf::from);
        invention
    }

    #[test]
    fn save_into_source_directory_keeps_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("original.json");
        let target = dir.path().join("copy.json");
        let invention = doc_with_refs(Some(&source.to_string_lossy()));

        invention.save_to_file(&target).unwrap();

        let saved = Invention::from_file(&target.to_string_lossy()).unwrap();
        assert_eq!(saved.developments[0].path.as_deref(), Some("voice.json"));
        assert_eq!(saved.assets["score"].path, "assets/score.json");
        assert_eq!(saved.title.as_deref(), Some("refs"));
    }

    #[test]
    fn save_into_other_directory_rebases_to_absolute() {
        let source_dir = tempfile::tempdir().unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("original.json");
        let target = target_dir.path().join("copy.json");
        let invention = doc_with_refs(Some(&source.to_string_lossy()));

        invention.save_to_file(&target).unwrap();

        let saved = Invention::from_file(&target.to_string_lossy()).unwrap();
        let dev_path = PathBuf::from(saved.developments[0].path.as_deref().unwrap());
        assert!(dev_path.is_absolute(), "{}", dev_path.display());
        assert!(dev_path.ends_with("voice.json"));
        let asset_path = PathBuf::from(&saved.assets["score"].path);
        assert!(asset_path.is_absolute(), "{}", asset_path.display());
    }

    #[test]
    fn save_without_source_path_writes_paths_as_authored() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("copy.json");
        let invention = doc_with_refs(None);

        invention.save_to_file(&target).unwrap();

        let saved = Invention::from_file(&target.to_string_lossy()).unwrap();
        assert_eq!(saved.developments[0].path.as_deref(), Some("voice.json"));
    }

    #[test]
    fn save_into_subdirectory_of_source_rebases_relative() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("original.json");
        let target = dir.path().join("nested/copy.json");
        let invention = doc_with_refs(Some(&source.to_string_lossy()));

        invention.save_to_file(&target).unwrap();

        // The referenced files live in the parent of the target directory,
        // which is not under it, so the rebased paths must be absolute.
        let saved = Invention::from_file(&target.to_string_lossy()).unwrap();
        let dev_path = PathBuf::from(saved.developments[0].path.as_deref().unwrap());
        assert!(dev_path.is_absolute(), "{}", dev_path.display());
    }
}
