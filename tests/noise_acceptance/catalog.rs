//! The shipped example invention.

use super::rig::SR;
use super::*;
use std::path::Path;

fn crate_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// Example file names listed in the example catalog.
/// `fugue::example_catalog` is a private test-only module, so the catalog is
/// read as source text: every `file_name: "..."` entry.
fn catalog_file_names() -> Vec<String> {
    crate_file("src/example_catalog.rs")
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("file_name:")?;
            Some(
                rest.trim()
                    .trim_end_matches(',')
                    .trim_matches('"')
                    .to_string(),
            )
        })
        .collect()
}

/// Criterion: "It ships an example invention under `examples/` listed in the
/// example catalog": some catalog entry names an `examples/` file that
/// exists, parses as an invention, and contains a module of type `noise`
/// whose config builds through the registry.
#[test]
fn catalog_lists_an_example_that_uses_noise() {
    let names = catalog_file_names();
    assert!(!names.is_empty(), "catalog source lists example files");
    let registry = ModuleRegistry::default();
    let mut found = Vec::new();
    for name in &names {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let doc: Value = serde_json::from_str(&text).unwrap_or_else(|err| panic!("{name}: {err}"));
        for module in doc["modules"].as_array().into_iter().flatten() {
            if module["type"] == "noise" {
                let config = module.get("config").cloned().unwrap_or_else(|| json!({}));
                registry
                    .build("noise", SR, &config)
                    .unwrap_or_else(|err| panic!("{name}: noise config {config}: {err}"));
                found.push(name.clone());
            }
        }
    }
    assert!(
        !found.is_empty(),
        "no cataloged example uses a noise module ({names:?})"
    );
    for name in &found {
        fugue::Invention::from_file(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("examples")
                .join(name)
                .to_str()
                .unwrap(),
        )
        .unwrap_or_else(|err| panic!("{name} loads as an invention: {err}"));
    }
}
