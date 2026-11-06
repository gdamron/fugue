//! Project definition of done: builds with no config, declares its controls
//! and ports, and ships an example invention.

use super::rig::*;
use super::*;
use std::path::Path;

fn number(v: &Voice, key: &str) -> (f32, f32, f32) {
    let meta = v
        .control(key)
        .unwrap_or_else(|| panic!("control {key} is declared"));
    let ControlValue::Number(default) = meta.default else {
        panic!("{key} default is a number");
    };
    let ControlKind::Number { min, max, .. } = meta.kind else {
        panic!("{key} is a number control");
    };
    (min, max, default)
}

/// Criterion: "the module builds with no config", and reports its ports:
/// inputs `frequency`, `frequency_mod`, `trigger`, `level`; outputs `audio`,
/// `aux`.
#[test]
fn builds_with_no_config_and_has_the_specified_ports() {
    let built = ModuleRegistry::default()
        .build("macro_oscillator", SR, &json!({}))
        .expect("builds with an empty config");
    let m = built.module.module();
    for name in ["frequency", "frequency_mod", "trigger", "level"] {
        assert!(
            m.inputs().contains(&name),
            "input {name} in {:?}",
            m.inputs()
        );
    }
    for name in ["audio", "aux"] {
        assert!(
            m.outputs().contains(&name),
            "output {name} in {:?}",
            m.outputs()
        );
    }
}

/// Criterion: "every control is declared (`controls()` lists every key
/// above)", with the ranges and defaults the spec gives.
#[test]
fn declares_controls_with_specified_ranges_and_defaults() {
    let v = voice(json!({}));
    assert_eq!(number(&v, "frequency").2, 440.0);
    let (min, max, default) = number(&v, "frequency_mod_depth");
    assert_eq!((min, max, default), (0.0, 1000.0, 0.0));
    for key in ["harmonics", "timbre", "morph"] {
        assert_eq!(number(&v, key), (0.0, 1.0, 0.5), "{key} range and default");
    }
    assert_eq!(number(&v, "level"), (0.0, 1.0, 1.0));
    let (min, max, _) = number(&v, "decay");
    assert!(min >= 0.0 && max > min, "decay range {min}..{max}");
    assert!(v.control("model").is_some());
}

/// Criterion: "`model`: a choice ... the full set, in this fixed order";
/// the part-1 models come first, in that order, starting with
/// `virtual_analog`. Later models may follow them, so `MODEL_ORDER` must be a
/// prefix of the reported options. Also: default is the first model and each
/// reported position is selectable and renders.
#[test]
fn model_options_are_a_prefix_of_the_fixed_order() {
    let mut v = voice(json!({}));
    assert!(
        v.models.len() >= MODEL_ORDER.len(),
        "models {:?} lack the part-1 models",
        v.models
    );
    assert_eq!(v.models[..MODEL_ORDER.len()], MODEL_ORDER[..], "order");
    assert_eq!(v.models[0], "virtual_analog");
    for position in 0..v.models.len() {
        v.select(position);
        v.render(BLOCK);
    }
}

/// Criterion: "config keys with the same names set the starting values".
/// Assumption: `model` takes the option's name as a string in config, as for
/// other choice controls.
#[test]
fn config_sets_starting_model_and_values() {
    let names = voice(json!({})).models;
    let last = names.last().unwrap().clone();
    let mut v = voice(json!({ "model": last, "frequency": 220.0, "harmonics": 0.25 }));
    assert_eq!(v.module.module().get_control("frequency"), Ok(220.0));
    assert_eq!(v.module.module().get_control("harmonics"), Ok(0.25));
    let x = v.render(seconds(0.5));
    assert!(
        peak(&x) > 0.001,
        "starts sounding on the configured model {last}"
    );
}

/// Criterion: every declared control accepts both ends of its declared range
/// (numbers min and max, choices every option), including `key` and `scale`
/// when the chords engine is present, and the module keeps producing
/// bounded audio afterwards.
#[test]
fn every_declared_control_accepts_its_whole_range() {
    let mut v = voice(json!({}));
    for meta in v.controls.clone() {
        match meta.kind {
            ControlKind::Number { min, max, .. } => {
                for value in [min, max] {
                    v.set(&meta.key, value);
                    let got = v.module.module().get_control(&meta.key).unwrap();
                    assert!(
                        (got - value).abs() <= 1e-3 * value.abs().max(1.0),
                        "{} set to {value} reads {got}",
                        meta.key
                    );
                }
            }
            ControlKind::String { options: Some(o) } => {
                for position in 0..o.len() {
                    v.set(&meta.key, position as f32);
                }
            }
            _ => {}
        }
        let x = v.render(BLOCK);
        assert!(
            x.iter().all(|s| s.is_finite() && s.abs() <= 1.0),
            "{} range",
            meta.key
        );
    }
}

/// Criterion: "an example invention under `examples/` uses
/// `macro_oscillator`": some `examples/*.json` has a module of that type,
/// whose config builds and which loads as an invention.
#[test]
fn an_example_invention_uses_macro_oscillator() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let registry = ModuleRegistry::default();
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("examples/ exists") {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        for module in doc["modules"].as_array().into_iter().flatten() {
            if module["type"] == "macro_oscillator" {
                let config = module.get("config").cloned().unwrap_or_else(|| json!({}));
                registry
                    .build("macro_oscillator", SR, &config)
                    .unwrap_or_else(|err| panic!("{}: config {config}: {err}", path.display()));
                found.push(path.clone());
            }
        }
    }
    assert!(
        !found.is_empty(),
        "no examples/*.json uses macro_oscillator"
    );
    for path in found {
        fugue::Invention::from_file(path.to_str().unwrap())
            .unwrap_or_else(|err| panic!("{} loads as an invention: {err}", path.display()));
    }
}
