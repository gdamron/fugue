//! Config is closed: a build refuses a key its type neither declares nor
//! takes as a control, naming the module and the key. Every first-party
//! example document still builds through a validation build.
//!
//! `FUGUE_DOCUMENT_DIRS` (paths joined by `:`) adds directories to sweep, so
//! a rename can check its sibling repos' documents locally:
//! `FUGUE_DOCUMENT_DIRS=../fugue-packs cargo test --lib closed`.

use crate::{Invention, InventionBuilder, ModuleRegistry};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[test]
fn an_undeclared_config_key_is_refused_naming_the_module_and_key() {
    let document = Invention::from_json(
        r#"{"version": "1.0.0", "connections": [],
            "modules": [{"id": "voice", "type": "oscillator", "config": {"waveform": "sawtooth"}}]}"#,
    )
    .unwrap();
    let registry = ModuleRegistry::default().for_validation();
    let error = InventionBuilder::with_registry(48_000, registry)
        .build(document)
        .err()
        .expect("an undeclared key is refused")
        .to_string();
    assert!(
        error.contains("module 'voice'")
            && error.contains("oscillator config has no key 'waveform'"),
        "{error}"
    );
    // Control keys are config keys too.
    let config: Value = json!({ "type": "sawtooth", "frequency": 220.0 });
    assert!(ModuleRegistry::default()
        .build("oscillator", 48_000, &config)
        .is_ok());
}

#[test]
fn a_sink_refuses_an_undeclared_key_before_it_opens_its_file() {
    let path = std::env::temp_dir().join(format!("fugue-closed-config-{}.wav", std::process::id()));
    let config = json!({ "path": path.to_string_lossy(), "max_secs": 1 });
    let error = ModuleRegistry::default()
        .build("audio_file_sink", 48_000, &config)
        .err()
        .expect("an undeclared key is refused")
        .to_string();
    assert!(
        error.contains("audio_file_sink config has no key 'max_secs'"),
        "{error}"
    );
    assert!(!path.exists(), "the sink opened its file");
}

/// The `.json` documents under `dir`, recursively, skipping score files
/// (`fugue.score.v1` is notation, not an invention).
fn documents(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.map(|entry| entry.unwrap().path()) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() && !name.starts_with('.') && name != "target" && name != "node_modules" {
            documents(&path, found);
        } else if name.ends_with(".json") && !name.contains("score") {
            found.push(path);
        }
    }
}

/// Why the invention at `path` does not build, if it doesn't. Files that
/// are not inventions (manifests, fixtures of other formats) are skipped.
fn refusal(path: &Path) -> Option<String> {
    let invention = Invention::from_file(path.to_str()?).ok()?;
    if invention.modules.is_empty() {
        return None;
    }
    let registry = ModuleRegistry::default().for_validation();
    InventionBuilder::with_registry(48_000, registry)
        .build(invention)
        .err()
        .map(|error| format!("{}: {error}", path.display()))
}

#[test]
fn every_example_document_builds() {
    let mut dirs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")];
    if let Ok(extra) = std::env::var("FUGUE_DOCUMENT_DIRS") {
        dirs.extend(extra.split(':').map(PathBuf::from));
    }
    let mut found = Vec::new();
    dirs.iter().for_each(|dir| documents(dir, &mut found));
    assert!(found.len() > 10, "found only {found:?}");
    let refused: Vec<String> = found.iter().filter_map(|path| refusal(path)).collect();
    assert!(refused.is_empty(), "{}", refused.join("\n"));
}
