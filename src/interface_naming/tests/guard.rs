//! The registry-wide interface convention guard (ledger
//! `module-interface-conventions`; section 4 of the convention spec). It
//! builds every built-in type and checks the names it exposes: its inputs,
//! outputs, declared controls, audio-thread control table and declared
//! config keys.
//!
//! A broken rule fails the test unless [`CONVENTION_EXCEPTIONS`] excuses it,
//! and an exception that excuses nothing fails it too, so the list only
//! shrinks: each rename removes its rows.
//!
//! Not checked yet: whether an input sharing a control's name overrides it,
//! and units and roles (seconds unsuffixed, `_beats`, verbs only for
//! events), which wait for the typed control tables to carry them. Nested
//! config fields (`zones[].root`) are not reached.

use super::exceptions::{CONVENTION_EXCEPTIONS, EXCEPTION_CEILING};
use crate::interface_naming::naming_problems;
use crate::module_config::tests::probe::ProbeFactory;
use crate::module_config::tests::registry::{eight_frame_wav, UNBUILDABLE};
use crate::ModuleRegistry;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Where a type exposes a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Input,
    Output,
    Control,
    /// The module's audio-thread control table (`Module::controls`).
    Table,
    Config,
}

/// A broken rule: the type, the name it is filed under (see [`family`]),
/// and why.
type Violation = (String, String, String);

/// The name a violation is filed under, so one exception covers a whole
/// family: `level.3` is `level.N`, and `in12` (digits glued on) is `in<N>`.
fn family(name: &str) -> String {
    if let Some((stem, _)) = name.split_once('.') {
        return format!("{stem}.N");
    }
    let word = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let last = word.rsplit('_').next().unwrap_or(word);
    let glued = word.len() < name.len() && !last.is_empty() && last != "x" && last != "d";
    if glued {
        format!("{word}<N>")
    } else {
        name.to_string()
    }
}

/// The config `type_id` is built from: the config harness's, with a sample
/// slot for the sample modules so their indexed controls exist.
fn base_config(type_id: &str) -> Value {
    let slot = json!({ "key": 36, "root": 60, "asset": { "path": eight_frame_wav() } });
    match type_id {
        "sample_kit" => json!({ "samples": [slot] }),
        "sample_instrument" => json!({ "zones": [slot] }),
        _ => crate::module_config::tests::registry::base_config(type_id),
    }
}

/// The names `type_id` exposes, built from [`base_config`].
fn interface(registry: &ModuleRegistry, type_id: &str) -> Result<Vec<(Kind, String)>, String> {
    let built = registry
        .for_validation()
        .build(type_id, 48_000, &base_config(type_id))
        .map_err(|error| error.to_string())?;
    let module = built.module.module();
    let mut names: Vec<(Kind, String)> = Vec::new();
    names.extend(module.inputs().iter().map(|n| (Kind::Input, n.to_string())));
    names.extend(
        module
            .outputs()
            .iter()
            .map(|n| (Kind::Output, n.to_string())),
    );
    if let Some(surface) = &built.control_surface {
        names.extend(
            surface
                .controls()
                .into_iter()
                .map(|m| (Kind::Control, m.key)),
        );
    }
    names.extend(module.controls().into_iter().map(|m| (Kind::Table, m.key)));
    let config = registry.config_keys(type_id).iter();
    names.extend(config.map(|key| (Kind::Config, key.key.to_string())));
    Ok(names)
}

/// Every rule `type_id` breaks.
fn check_type(registry: &ModuleRegistry, type_id: &str) -> Vec<Violation> {
    let mut found = BTreeSet::new();
    let mut push = |name: &str, why: String| {
        found.insert((type_id.to_string(), family(name), why));
    };
    let names = match interface(registry, type_id) {
        Ok(names) => names,
        Err(error) => {
            push("*", format!("base config refused: {error}"));
            return found.into_iter().collect();
        }
    };
    let of = |kind: Kind| -> BTreeSet<&str> {
        names
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, n)| n.as_str())
            .collect()
    };
    let controls = of(Kind::Control);

    // Spelling and banned tokens (N1, N2, N5, N6, N8).
    for (kind, name) in &names {
        for problem in naming_problems(name) {
            push(name, format!("{kind:?} '{}' {problem}", family(name)));
        }
    }
    // Indices run from 0 with no gaps (N4).
    for kind in [Kind::Input, Kind::Output, Kind::Control] {
        let mut families: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for name in of(kind) {
            if let Some((stem, index)) = name.split_once('.') {
                let index = index.parse().unwrap_or(usize::MAX);
                families.entry(stem).or_default().push(index);
            }
        }
        for (stem, mut indices) in families {
            indices.sort_unstable();
            if indices.iter().enumerate().any(|(n, index)| n != *index) {
                push(
                    &format!("{stem}.N"),
                    format!("{kind:?} indices skip or miss 0"),
                );
            }
        }
    }
    // Pairs (N8): left with right, a modulation input with its depth.
    for kind in [Kind::Input, Kind::Output] {
        let ports = of(kind);
        for (side, other) in [("_left", "_right"), ("_right", "_left")] {
            for port in &ports {
                if let Some(stem) = port.strip_suffix(side) {
                    if !ports.contains(format!("{stem}{other}").as_str()) {
                        push(port, format!("{kind:?} '{port}' has no {stem}{other}"));
                    }
                }
            }
        }
    }
    let inputs = of(Kind::Input);
    for input in &inputs {
        if input.ends_with("_mod") && !controls.contains(format!("{input}_depth").as_str()) {
            push(
                input,
                format!("input '{input}' has no {input}_depth control"),
            );
        }
    }
    for control in &controls {
        if let Some(input) = control.strip_suffix("_depth") {
            if input.ends_with("_mod") && !inputs.contains(input) {
                push(control, format!("control '{control}' has no {input} input"));
            }
        }
    }
    // Config keys and control keys (N9).
    let stems: BTreeSet<&str> = controls
        .iter()
        .filter_map(|c| c.split_once('.'))
        .map(|c| c.0)
        .collect();
    for key in of(Kind::Config)
        .into_iter()
        .filter(|key| !controls.contains(key))
    {
        let respelled = controls.iter().find(|control| {
            key == format!("{type_id}_{control}")
                || key.strip_suffix("_json") == Some(control)
                || key.strip_suffix("_enabled") == Some(control)
                || **control == format!("{key}_json")
        });
        if let Some(control) = respelled {
            push(key, format!("config '{key}' respells control '{control}'"));
        }
        for stem in &stems {
            if key.ends_with(&format!("_{stem}s")) {
                push(
                    key,
                    format!("config '{key}' is the array of {stem}.N, so is {stem}s"),
                );
            }
        }
    }
    // One table: the audio-thread table declares nothing the controls don't.
    for name in of(Kind::Table).difference(&controls) {
        push(
            name,
            format!("'{name}' is in the audio-thread table, not the controls"),
        );
    }
    // Closed config: an undeclared key is refused, naming type and key.
    let mut config = base_config(type_id);
    config["not_a_declared_key"] = json!(0);
    match registry.for_validation().build(type_id, 48_000, &config) {
        Err(error)
            if error
                .to_string()
                .contains(&format!("{type_id} config has no key 'not_a_declared_key'")) => {}
        Err(error) => push("*", format!("an undeclared key gave: {error}")),
        Ok(_) => push("*", "config is open: an undeclared key builds".to_string()),
    }
    found.into_iter().collect()
}

/// Every rule a type of `registry` breaks.
fn check_registry(registry: &ModuleRegistry) -> Vec<Violation> {
    let mut types: Vec<&str> = registry.types().collect();
    types.sort_unstable();
    types
        .into_iter()
        .filter(|type_id| UNBUILDABLE.iter().all(|(id, _)| id != type_id))
        .flat_map(|type_id| check_type(registry, type_id))
        .collect()
}

fn excused(violation: &Violation) -> bool {
    CONVENTION_EXCEPTIONS
        .iter()
        .any(|(type_id, name, _)| *type_id == violation.0 && *name == violation.1)
}

#[test]
fn every_module_type_follows_the_interface_convention() {
    let violations = check_registry(&ModuleRegistry::default());
    let unexcused: Vec<String> = violations
        .iter()
        .filter(|violation| !excused(violation))
        .map(|(type_id, name, why)| format!("(\"{type_id}\", \"{name}\"): {why}"))
        .collect();
    let stale: Vec<String> = CONVENTION_EXCEPTIONS
        .iter()
        .filter(|(type_id, name, _)| !violations.iter().any(|v| v.0 == *type_id && v.1 == *name))
        .map(|(type_id, name, _)| format!("(\"{type_id}\", \"{name}\")"))
        .collect();
    assert!(
        unexcused.is_empty() && stale.is_empty(),
        "Broken rules (rename to the convention; never add an exception):\n{}\n\n\
         Exceptions that excuse nothing (delete these rows):\n{}",
        unexcused.join("\n"),
        stale.join("\n"),
    );
}

#[test]
fn the_exception_list_only_shrinks() {
    assert!(
        CONVENTION_EXCEPTIONS.len() <= EXCEPTION_CEILING,
        "the exception list grew past its ceiling; rename instead"
    );
    let rows: BTreeSet<(&str, &str)> = CONVENTION_EXCEPTIONS
        .iter()
        .map(|(t, n, _)| (*t, *n))
        .collect();
    assert_eq!(
        rows.len(),
        CONVENTION_EXCEPTIONS.len(),
        "an exception is listed twice"
    );
}

#[test]
fn the_guard_catches_a_type_that_breaks_the_rules() {
    let mut registry = ModuleRegistry::new();
    registry.register(ProbeFactory);
    let names: BTreeSet<String> = check_registry(&registry).into_iter().map(|v| v.1).collect();
    // The probe is an oscillator with an `hz` key, which names a unit
    // rather than the musical quantity.
    assert!(names.contains("hz"), "hz not caught in {names:?}");
}
