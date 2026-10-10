//! First-party developments' exposed names follow the convention too
//! (N12): the inputs, outputs and controls a development exposes, whether
//! the document is one or defines one inline (nested ones included). The
//! documents are the closed-config sweep's: `examples/` plus
//! `FUGUE_DOCUMENT_DIRS`. A development referenced by path or ref is
//! checked where its own document is swept.
//!
//! No exception list: every first-party development conforms, and a new
//! one must.

use crate::interface_naming::naming_problems;
use crate::module_config::tests::closed::{first_party_documents, parse};
use crate::Invention;
use std::collections::BTreeMap;

/// The names `document` exposes, as (where, kind, name), with each inline
/// development's under its own `where`.
fn exposed(document: &Invention, at: &str, found: &mut Vec<(String, &'static str, String)>) {
    let mut push = |kind, name: &str| found.push((at.to_string(), kind, name.to_string()));
    document.inputs.iter().for_each(|i| push("input", &i.name));
    document
        .outputs
        .iter()
        .for_each(|o| push("output", &o.name));
    document
        .controls
        .iter()
        .for_each(|c| push("control", &c.name));
    for development in &document.developments {
        if let Some(definition) = &development.definition {
            exposed(definition, &format!("{at} > {}", development.name), found);
        }
    }
}

/// Every rule the developments in `document` break, one line each.
fn violations(document: &Invention, at: &str) -> Vec<String> {
    let mut names = Vec::new();
    exposed(document, at, &mut names);
    let mut found: Vec<String> = Vec::new();
    // Indices of each (where, kind, stem) family, which run from 0 (N4).
    let mut families: BTreeMap<(&str, &str, &str), Vec<usize>> = BTreeMap::new();
    for (at, kind, name) in &names {
        for problem in naming_problems(name) {
            found.push(format!("{at}: {kind} '{name}' {problem}"));
        }
        if let Some((stem, index)) = name.split_once('.') {
            let index = index.parse().unwrap_or(usize::MAX);
            families.entry((at, kind, stem)).or_default().push(index);
        }
    }
    for ((at, kind, stem), mut indices) in families {
        indices.sort_unstable();
        indices.dedup(); // a control name listed twice fans out
        if indices.iter().enumerate().any(|(n, index)| n != *index) {
            found.push(format!("{at}: {kind} '{stem}.N' indices skip or miss 0"));
        }
    }
    found
}

#[test]
fn every_first_party_development_exposes_conforming_names() {
    let mut found = Vec::new();
    for path in first_party_documents() {
        match parse(&path) {
            None => {}
            Some(Err(error)) => found.push(error),
            Some(Ok(document)) => found.extend(violations(&document, &path.display().to_string())),
        }
    }
    assert!(
        found.is_empty(),
        "Developments expose names that break the convention (N12):\n{}",
        found.join("\n")
    );
}

#[test]
fn the_development_guard_catches_a_nonconforming_name() {
    let voice = r#"{"modules": [{"id": "f", "type": "filter"}], "connections": [],
        "inputs": [{"name": "in1", "to": "f", "to_port": "audio"}],
        "outputs": [{"name": "audio", "from": "f", "from_port": "audio"}],
        "controls": [{"name": "cutoff_cv", "module": "f", "control": "cutoff"},
                     {"name": "level.1", "module": "f", "control": "resonance"}]}"#;
    let document = format!(
        r#"{{"modules": [], "connections": [],
            "developments": [{{"name": "voice", "definition": {voice}}}]}}"#
    );
    let found = violations(&Invention::from_json(&document).unwrap(), "doc").join("\n");
    for expected in [
        "doc > voice: input 'in1' glues digits",
        "doc > voice: control 'cutoff_cv' uses 'cv'",
        "doc > voice: control 'level.N' indices skip or miss 0",
    ] {
        assert!(found.contains(expected), "{expected} not in:\n{found}");
    }
    assert!(!found.contains("'audio'"), "{found}");
}

#[test]
fn a_control_named_by_the_old_key_field_does_not_parse() {
    let voice = r#"{"modules": [{"id": "o", "type": "oscillator"}], "connections": [],
        "controls": [{"key": "waveform", "module": "o", "control": "waveform"}]}"#;
    let error = Invention::from_json(voice).err().unwrap().to_string();
    assert!(error.contains("missing field `name`"), "{error}");
}
