//! The installed packages an invention reaches when it loads, so a frozen load
//! verifies only those (see `Lockfile::verify_first_for_invention`).

use super::*;
use crate::invention::audio_assets::module_asset_values;
use crate::invention::builder::{resolve_development_path, resolve_json_assets};
use std::collections::BTreeSet;

/// Development nesting bound, matching the builder and the content walk.
const MAX_DEPTH: usize = 64;

/// A `code` module's script can set a module's `asset` control to any package
/// ref at runtime (`setControl`), which no document walk can see.
const SCRIPT_TYPE: &str = "code";

/// What the walk has found so far: package ids, and the development files
/// already read, so a cycle or a shared development is walked once.
#[derive(Default)]
struct Walk {
    used: BTreeSet<String>,
    visited: BTreeSet<PathBuf>,
}

impl ContentRoots {
    /// Package ids `invention` uses: package audio refs in module configs
    /// (after `$asset` expansion) and development `ref`s, through every nested
    /// development whether it comes by path, ref or inline definition.
    ///
    /// `None` when some document or reference cannot be read, or a script
    /// could choose packages at runtime: the caller then cannot know what was
    /// skipped, so it must verify everything. Reading documents never hashes
    /// package contents.
    pub fn used_packages(&self, invention: &Invention) -> Option<BTreeSet<String>> {
        let mut walk = Walk::default();
        self.collect_used(invention, &mut walk, 0)?;
        Some(walk.used)
    }

    fn collect_used(&self, document: &Invention, walk: &mut Walk, depth: usize) -> Option<()> {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut expanded = document.clone();
        resolve_json_assets(&mut expanded).ok()?;
        for module in &mut expanded.modules {
            if module.module_type == SCRIPT_TYPE {
                return None;
            }
            for value in module_asset_values(&mut module.config) {
                if let Some(reference) = value.as_str().and_then(PackageAudioRef::parse) {
                    walk.used.insert(reference.id);
                }
            }
        }
        for development in &document.developments {
            let file = if let Some(reference) = &development.reference {
                if let ContentRef::Package { package, .. } = reference {
                    walk.used.insert(package.clone());
                }
                self.reference_path(reference).ok()?.0
            } else if let Some(path) = &development.path {
                resolve_development_path(document.source_path.as_deref(), path).ok()?
            } else {
                let mut child = development.definition.as_deref()?.clone();
                child.source_path = document.source_path.clone();
                self.collect_used(&child, walk, depth + 1)?;
                continue;
            };
            if walk.visited.insert(std::fs::canonicalize(&file).ok()?) {
                let child = Invention::from_file(file.to_str()?).ok()?;
                self.collect_used(&child, walk, depth + 1)?;
            }
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
                    sampler(json!({"path": "local.wav"})),
                    {"id": "kit", "type": "sample_kit", "config": {"samples": [
                        {"asset": "fugue.demo.kit@1:snare.wav"}]}},
                    {"id": "inst", "type": "sample_instrument", "config": {"zones": [
                        {"asset": "fugue.demo.piano@1:a0.wav"}]}}],
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
                "fugue.demo.kit",
                "fugue.demo.pads",
                "fugue.demo.piano",
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

    #[test]
    fn a_script_makes_the_set_unknown() {
        let (temp, roots) = roots();
        let source = temp.path().join("main.json");
        write(
            &temp.path().join("voice.json"),
            &json!({"modules": [{"id": "c", "type": "code", "config": {}}], "connections": []}),
        );
        let main = invention(
            json!({"modules": [], "connections": [],
                "developments": [{"name": "voice", "path": "voice.json"}]}),
            &source,
        );
        assert_eq!(roots.used_packages(&main), None);
    }

    #[test]
    fn cyclic_and_shared_developments_are_walked_once() {
        let (temp, roots) = roots();
        let source = temp.path().join("main.json");
        // `loop.json` declares itself twice: without a visited set this
        // walk would branch twice per level down to the depth bound.
        write(
            &temp.path().join("loop.json"),
            &json!({"modules": [sampler(json!("fugue.demo.loop@1:l.wav"))], "connections": [],
                "developments": [{"name": "a", "path": "loop.json"},
                    {"name": "b", "path": "loop.json"}]}),
        );
        let main = invention(
            json!({"modules": [], "connections": [],
                "developments": [{"name": "a", "path": "loop.json"},
                    {"name": "b", "path": "loop.json"}]}),
            &source,
        );
        assert_eq!(
            roots
                .used_packages(&main)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["fugue.demo.loop"]
        );
    }
}
