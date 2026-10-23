//! The installed packages an invention reaches when it loads, so a frozen load
//! verifies only those (see `Lockfile::verify_first_for_invention`).

use super::*;
use crate::invention::builder::{resolve_development_path, resolve_json_assets};
use std::collections::BTreeSet;

/// Development nesting bound, matching the builder and the content walk.
const MAX_DEPTH: usize = 64;

/// A `code` module's script can set a module's `asset` control to any package
/// ref at runtime (`setControl`), which no document walk can see.
const SCRIPT_TYPE: &str = crate::modules::code::TYPE_ID;

/// What the walk has found so far: package ids, and the development files
/// already read, so a cycle or a shared development is walked once. A file is
/// keyed with the directory it is read from: the builder resolves its relative
/// `$asset`s and developments from there, so one file reached through two
/// directories (a symlink) can load different packages from each.
#[derive(Default)]
struct Walk {
    used: BTreeSet<String>,
    visited: BTreeSet<(PathBuf, PathBuf)>,
}

impl ContentRoots {
    /// Package ids `invention` uses: package audio refs in module configs
    /// (after `$asset` expansion) and development `ref`s, through every nested
    /// development whether it comes by path, ref or inline definition.
    ///
    /// A development instance's config sets its exposed controls, and an
    /// exposed control can alias a sample module's `asset` under any name
    /// (`{"sample": "fugue.drums@1:kick.wav"}`), through any depth of nested
    /// developments. Each alias hands the string on unchanged, and every
    /// package-loading sink takes a package only from a string that parses as
    /// a [`PackageAudioRef`], including a local `{"path": ...}` object that
    /// flattens to one. So every string anywhere in a module config that
    /// parses as one counts as used, whatever its key or depth: a superset of
    /// the alias targets and asset slots that needs no tracing of where an
    /// alias leads and cannot miss one. A string nothing loads only costs one
    /// more hash.
    ///
    /// `None` when some document or reference cannot be read, a document's
    /// path could join a local sample path into a package ref, a `$asset`
    /// expands to another `$asset` (the builder expands again, so its string
    /// would only appear later), or a script could choose packages at
    /// runtime: the caller then cannot know what was skipped, so it must
    /// verify everything. Reading documents never hashes package contents.
    pub fn used_packages(&self, invention: &Invention) -> Option<BTreeSet<String>> {
        let mut walk = Walk::default();
        self.collect_used(invention, &mut walk, 0)?;
        Some(walk.used)
    }

    fn collect_used(&self, document: &Invention, walk: &mut Walk, depth: usize) -> Option<()> {
        if depth > MAX_DEPTH {
            return None;
        }
        // The builder joins a local sample path onto the document's
        // directory, and the sample loader reads a package ref from the
        // result: a directory like `fugue.drums@1:kit` turns `kick.wav` into
        // one, which no string in the config shows. A ref's `@` must then
        // come from the directory (one from the sample path would leave a
        // `/` in the package id), so any `@` there makes the set unknown.
        let source = document.source_path.as_deref().map(Path::to_string_lossy);
        if source.is_some_and(|text| text.contains('@')) {
            return None;
        }
        let mut expanded = document.clone();
        resolve_json_assets(&mut expanded).ok()?;
        for module in &expanded.modules {
            if module.module_type == SCRIPT_TYPE {
                return None;
            }
            collect_refs(&module.config, &mut walk.used)?;
        }
        for development in &document.developments {
            let file = if let Some(reference) = &development.reference {
                if let ContentRef::Package { package, .. } = reference {
                    walk.used.insert(package.clone());
                }
                self.reference_path(reference, Purpose::Browse).ok()?.0
            } else if let Some(path) = &development.path {
                resolve_development_path(document.source_path.as_deref(), path).ok()?
            } else {
                let mut child = development.definition.as_deref()?.clone();
                child.source_path = document.source_path.clone();
                self.collect_used(&child, walk, depth + 1)?;
                continue;
            };
            // Before `visited`: a spelling with an `@` can reach a file
            // already walked through a symlink, and only the spelling
            // decides whether a joined local sample path parses as a ref.
            if file.to_string_lossy().contains('@') {
                return None;
            }
            let context = std::fs::canonicalize(file.parent()?).ok()?;
            if walk
                .visited
                .insert((std::fs::canonicalize(&file).ok()?, context))
            {
                let child = Invention::from_file(file.to_str()?).ok()?;
                self.collect_used(&child, walk, depth + 1)?;
            }
        }
        Some(())
    }
}

/// Adds the package id of every string in `value`, at any depth, that parses
/// as a [`PackageAudioRef`]. `None` on a `$asset` left after expansion.
fn collect_refs(value: &serde_json::Value, used: &mut BTreeSet<String>) -> Option<()> {
    match value {
        serde_json::Value::String(text) => {
            if let Some(reference) = PackageAudioRef::parse(text) {
                used.insert(reference.id);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_refs(item, used)?;
            }
        }
        serde_json::Value::Object(map) => {
            if map.contains_key("$asset") {
                return None;
            }
            for item in map.values() {
                collect_refs(item, used)?;
            }
        }
        _ => {}
    }
    Some(())
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

    /// A development document exposing its sample player's `asset` as
    /// `sample`, as Codex's repro does.
    fn drums(nested: serde_json::Value) -> serde_json::Value {
        json!({"modules": [{"id": "s", "type": "sample_player", "config": {}}, nested],
            "connections": [], "controls": [{"name": "sample", "module": "s", "control": "asset"}]})
    }

    #[test]
    fn collects_packages_chosen_through_a_development_control_alias() {
        let (temp, roots) = roots();
        let main = invention(
            json!({"modules": [{"id": "d", "type": "drums",
                    "config": {"sample": "fugue.drums@1:kick.wav", "mode": "loop", "gain": 0.5}}],
                "connections": [],
                "developments": [{"name": "drums", "definition": drums(json!(
                    {"id": "n", "type": "sample_player", "config": {"asset": {"path": "a.wav"}}}))}]}),
            &temp.path().join("main.json"),
        );
        let used = roots.used_packages(&main).unwrap();
        assert_eq!(used.into_iter().collect::<Vec<_>>(), ["fugue.drums"]);
    }

    #[test]
    fn collects_packages_chosen_through_nested_development_aliases() {
        let (temp, roots) = roots();
        // `kit` re-exposes `sample` from its inner `drums` instance, which
        // itself sets an inner alias in `kit.json`.
        write(
            &temp.path().join("kit.json"),
            &json!({"modules": [{"id": "inner", "type": "drums",
                    "config": {"sample": "fugue.demo.inner@1:y.wav"}}],
                "connections": [],
                "controls": [{"name": "sample", "module": "inner", "control": "sample"}],
                "developments": [{"name": "drums", "definition": drums(json!(
                    {"id": "n", "type": "drums", "config": {"sample": "fugue.demo.deep@^2:z.wav"}}))}]}),
        );
        let main = invention(
            json!({"modules": [{"id": "k", "type": "kit",
                    "config": {"sample": "fugue.demo.outer@1.0.0:x.wav"}}],
                "connections": [],
                "developments": [{"name": "kit", "path": "kit.json"}]}),
            &temp.path().join("main.json"),
        );
        let used = roots.used_packages(&main).unwrap();
        assert_eq!(
            used.into_iter().collect::<Vec<_>>(),
            ["fugue.demo.deep", "fugue.demo.inner", "fugue.demo.outer"]
        );
    }

    #[test]
    fn an_alias_into_an_unreadable_development_makes_the_set_unknown() {
        let (temp, roots) = roots();
        // The aliased instance's development is a package ref that is not
        // installed: where `sample` leads cannot be read, so it is unknown.
        let main = invention(
            json!({"modules": [{"id": "d", "type": "drums",
                    "config": {"sample": "fugue.drums@1:kick.wav"}}],
                "connections": [],
                "developments": [{"name": "drums",
                    "ref": {"package": "fugue.demo.missing", "version": "1.0.0"}}]}),
            &temp.path().join("main.json"),
        );
        assert_eq!(roots.used_packages(&main), None);
    }

    #[test]
    fn collects_a_package_ref_hidden_in_a_local_path_object() {
        let (temp, roots) = roots();
        // An inline development has no source path, so the builder flattens
        // this path object to the bare string, which then loads the package.
        let main = invention(
            json!({"modules": [{"id": "d", "type": "inline", "config": {}}],
                "connections": [],
                "developments": [{"name": "inline", "definition": {"modules": [
                    sampler(json!({"path": "fugue.hidden@1:x.wav"}))], "connections": []}}]}),
            &temp.path().join("main.json"),
        );
        let used = roots.used_packages(&main).unwrap();
        assert_eq!(used.into_iter().collect::<Vec<_>>(), ["fugue.hidden"]);
    }

    #[test]
    fn a_chained_asset_makes_the_set_unknown() {
        let (temp, roots) = roots();
        // `a.json` holds another `$asset`, which only the builder's second
        // expansion turns into the package ref in `b.json`.
        write(&temp.path().join("a.json"), &json!({"$asset": "b"}));
        write(&temp.path().join("b.json"), &json!("fugue.hidden@1:x.wav"));
        let main = invention(
            json!({"assets": {"a": {"path": "a.json"}, "b": {"path": "b.json"}},
                "modules": [{"id": "d", "type": "drums", "config": {"sample": {"$asset": "a"}}}],
                "connections": []}),
            &temp.path().join("main.json"),
        );
        assert_eq!(roots.used_packages(&main), None);
    }

    #[test]
    fn a_package_shaped_document_directory_makes_the_set_unknown() {
        let (temp, roots) = roots();
        // `sample.wav` joins onto this directory as `...fugue.hidden@1:song/sample.wav`.
        let main = invention(
            json!({"modules": [sampler(json!({"path": "sample.wav"}))], "connections": []}),
            Path::new("fugue.hidden@1:song/main.json"),
        );
        assert_eq!(roots.used_packages(&main), None);
        let plain = invention(
            json!({"modules": [sampler(json!({"path": "sample.wav"}))], "connections": []}),
            &temp.path().join("song/main.json"),
        );
        assert_eq!(roots.used_packages(&plain), Some(BTreeSet::new()));
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

    #[cfg(unix)]
    #[test]
    fn one_document_reached_from_two_directories_is_walked_in_each() {
        let (temp, roots) = roots();
        // `a/voice.json` and `b/voice.json` link to one document whose
        // relative `$asset` names a different package beside each link.
        write(
            &temp.path().join("real/voice.json"),
            &json!({"assets": {"s": {"path": "sample.json"}},
                "modules": [sampler(json!({"$asset": "s"}))], "connections": []}),
        );
        for (dir, id) in [("a", "fugue.demo.a"), ("b", "fugue.demo.b")] {
            write(
                &temp.path().join(dir).join("sample.json"),
                &json!(format!("{id}@1:x.wav")),
            );
            std::os::unix::fs::symlink(
                temp.path().join("real/voice.json"),
                temp.path().join(dir).join("voice.json"),
            )
            .unwrap();
        }
        let main = invention(
            json!({"modules": [], "connections": [],
                "developments": [{"name": "a", "path": "a/voice.json"},
                    {"name": "b", "path": "b/voice.json"}]}),
            &temp.path().join("main.json"),
        );
        let used = roots.used_packages(&main).unwrap();
        assert_eq!(
            used.into_iter().collect::<Vec<_>>(),
            ["fugue.demo.a", "fugue.demo.b"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_package_shaped_spelling_of_a_walked_document_makes_the_set_unknown() {
        let (temp, roots) = roots();
        // `safe` is walked first; `fugue.hidden@1:kit` links to it, so its
        // `voice.json` is the same file in the same directory.
        write(
            &temp.path().join("safe/voice.json"),
            &json!({"modules": [sampler(json!({"path": "kick.wav"}))], "connections": []}),
        );
        std::os::unix::fs::symlink(
            temp.path().join("safe"),
            temp.path().join("fugue.hidden@1:kit"),
        )
        .unwrap();
        let main = invention(
            json!({"modules": [], "connections": [],
                "developments": [{"name": "a", "path": "safe/voice.json"},
                    {"name": "b", "path": "fugue.hidden@1:kit/voice.json"}]}),
            &temp.path().join("main.json"),
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
