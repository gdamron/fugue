//! The installed packages an invention reaches when it loads, so a frozen load
//! verifies only those (see `Lockfile::verify_first_for_invention`).

use super::*;
use crate::invention::audio_assets::module_asset_values;
use crate::invention::builder::{resolve_development_path, resolve_json_assets};
use std::collections::BTreeSet;

/// Development nesting bound, matching the builder and the content walk.
const MAX_DEPTH: usize = 64;

impl ContentRoots {
    /// Package ids `invention` uses: package audio refs in module configs
    /// (after `$asset` expansion) and development `ref`s, through every nested
    /// development whether it comes by path, ref or inline definition.
    ///
    /// `None` when some document or reference cannot be read: the caller
    /// then cannot know what was skipped, so it must verify everything.
    /// Reading documents never hashes package contents.
    pub fn used_packages(&self, invention: &Invention) -> Option<BTreeSet<String>> {
        let mut used = BTreeSet::new();
        self.collect_used(invention, &mut used, 0)?;
        Some(used)
    }

    fn collect_used(
        &self,
        document: &Invention,
        used: &mut BTreeSet<String>,
        depth: usize,
    ) -> Option<()> {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut expanded = document.clone();
        resolve_json_assets(&mut expanded).ok()?;
        for module in &mut expanded.modules {
            for value in module_asset_values(&mut module.config) {
                if let Some(reference) = value.as_str().and_then(PackageAudioRef::parse) {
                    used.insert(reference.id);
                }
            }
        }
        for development in &document.developments {
            let child = if let Some(reference) = &development.reference {
                if let ContentRef::Package { package, .. } = reference {
                    used.insert(package.clone());
                }
                let (path, _, _) = self.reference_path(reference).ok()?;
                Invention::from_file(path.to_str()?).ok()?
            } else if let Some(path) = &development.path {
                let path = resolve_development_path(document.source_path.as_deref(), path).ok()?;
                Invention::from_file(path.to_str()?).ok()?
            } else {
                let mut child = development.definition.as_deref()?.clone();
                child.source_path = document.source_path.clone();
                child
            };
            self.collect_used(&child, used, depth + 1)?;
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roots() -> (tempfile::TempDir, ContentRoots) {
        let temp = tempfile::tempdir().unwrap();
        let roots = ContentRoots {
            packages: temp.path().join("packs"),
            workspace: temp.path().join("inventions"),
        };
        (temp, roots)
    }

    fn write(path: &Path, value: &serde_json::Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    }

    fn invention(value: serde_json::Value, source: &Path) -> Invention {
        let mut invention: Invention = serde_json::from_value(value).unwrap();
        invention.source_path = Some(source.to_path_buf());
        invention
    }

    fn sampler(asset: serde_json::Value) -> serde_json::Value {
        json!({"id": "s", "type": "sample_player", "config": {"asset": asset}})
    }

    #[test]
    fn collects_audio_refs_through_assets_and_nested_developments() {
        let (temp, roots) = roots();
        // A package development whose own document plays a sample pack.
        let dev_root = roots.packages.join("fugue.demo.voice/1.0.0");
        write(
            &dev_root.join("fugue.pkg.json"),
            &json!({"id": "fugue.demo.voice", "version": "1.0.0", "kind": "development",
                "license": "MIT", "authors": [{"name": "Test"}], "targets": ["external-agent"],
                "entry": {"development": "voice.json"}}),
        );
        write(
            &dev_root.join("voice.json"),
            &json!({"modules": [sampler(json!("fugue.demo.strings@1:a.wav"))], "connections": []}),
        );
        // A path development that pulls its sample through a `$asset`.
        let source = temp.path().join("piece/main.json");
        write(
            &temp.path().join("piece/kick.json"),
            &json!("fugue.demo.drums@^1.2:kick.wav"),
        );
        write(
            &temp.path().join("piece/kit.json"),
            &json!({"assets": {"kick": {"path": "kick.json"}},
                "modules": [sampler(json!({"$asset": "kick"}))], "connections": []}),
        );
        let main = invention(
            json!({
                "modules": [sampler(json!("fugue.demo.keys@2.0.0:c4.wav")),
                    sampler(json!({"path": "local.wav"}))],
                "connections": [],
                "developments": [
                    {"name": "voice", "ref": {"package": "fugue.demo.voice", "version": "1.0.0"}},
                    {"name": "kit", "path": "kit.json"},
                    {"name": "inline", "definition": {
                        "modules": [sampler(json!("fugue.demo.pads@1:p.wav"))], "connections": []}}
                ]
            }),
            &source,
        );

        let used = roots.used_packages(&main).unwrap();
        assert_eq!(
            used.into_iter().collect::<Vec<_>>(),
            [
                "fugue.demo.drums",
                "fugue.demo.keys",
                "fugue.demo.pads",
                "fugue.demo.strings",
                "fugue.demo.voice"
            ]
        );
    }

    #[test]
    fn an_unreadable_development_makes_the_set_unknown() {
        let (temp, roots) = roots();
        let main = invention(
            json!({"modules": [], "connections": [],
                "developments": [{"name": "gone", "path": "missing.json"}]}),
            &temp.path().join("main.json"),
        );
        assert_eq!(roots.used_packages(&main), None);
    }
}
