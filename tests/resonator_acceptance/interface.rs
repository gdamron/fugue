//! Interface and definition of done: builds with `{}`, declares its controls
//! and ports, refuses unknown config, ships an example invention.

use super::rig::*;
use super::*;
use std::path::Path;

fn number(r: &Reso, key: &str) -> (f32, f32, f32) {
    let meta = r
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

/// Criterion: "builds from `{}`", with inputs `audio`, `frequency`, `strum`
/// and outputs `odd`, `even`, `audio`.
#[test]
fn builds_with_no_config_and_has_the_specified_ports() {
    let module = try_build(&json!({})).expect("builds with an empty config");
    let m = module.module();
    for name in ["audio", "frequency", "strum"] {
        assert!(
            m.inputs().contains(&name),
            "input {name} in {:?}",
            m.inputs()
        );
    }
    for name in ["odd", "even", "audio"] {
        assert!(
            m.outputs().contains(&name),
            "output {name} in {:?}",
            m.outputs()
        );
    }
}

/// Criterion: "unknown config key refused" (any unknown key, whatever the
/// value type, alongside valid ones too).
#[test]
fn unknown_config_keys_are_refused() {
    assert!(try_build(&json!({ "no_such_key": 1.0 })).is_err());
    assert!(try_build(&json!({ "no_such_key": "x" })).is_err());
    assert!(try_build(&json!({ "damping": 0.5, "no_such_key": true })).is_err());
    assert!(try_build(&json!({ "damping": 0.5 })).is_ok());
}

/// Criterion: "controls declared as in INTERFACE.md": `frequency` 20..20000
/// default 220; `structure`, `brightness`, `damping`, `position` 0..1 with
/// defaults 0.25, 0.5, 0.5, 0.3; `polyphony` 1..4 default 1; `level` 0..1
/// default 1; `model` with default `modal`.
/// Assumption: this must hold for the module as a whole, so it fails on a
/// stack PR that has not yet added every control.
#[test]
fn declares_controls_with_specified_ranges_and_defaults() {
    let r = reso(json!({}));
    assert_eq!(number(&r, "frequency"), (20.0, 20_000.0, 220.0));
    assert_eq!(number(&r, "structure"), (0.0, 1.0, 0.25));
    assert_eq!(number(&r, "brightness"), (0.0, 1.0, 0.5));
    assert_eq!(number(&r, "damping"), (0.0, 1.0, 0.5));
    assert_eq!(number(&r, "position"), (0.0, 1.0, 0.3));
    assert_eq!(number(&r, "polyphony"), (1.0, 4.0, 1.0));
    assert_eq!(number(&r, "level"), (0.0, 1.0, 1.0));
    let model = r.control("model").expect("model is declared");
    assert_eq!(model.default, ControlValue::String("modal".to_string()));
    assert_eq!(r.module.module().get_control("model"), Ok(0.0));
}

/// Criterion: `model` options "in order `modal`, `sympathetic_strings`,
/// `string`": models arrive across a stack of PRs, so the options present
/// must be a prefix of that order starting with `modal`; each position is
/// selectable and `set_control("model", i)` selects option i.
#[test]
fn model_options_are_a_prefix_of_the_fixed_order() {
    let mut r = reso(json!({}));
    assert!(!r.models.is_empty(), "at least one model");
    assert!(r.models.len() <= MODEL_ORDER.len(), "models {:?}", r.models);
    assert_eq!(r.models[..], MODEL_ORDER[..r.models.len()], "order");
    for position in 0..r.models.len() {
        r.select(position);
        r.render(BLOCK);
        assert_eq!(r.module.module().get_control("model"), Ok(position as f32));
    }
}

/// Criterion: "every config key is a control's starting value": config sets
/// the starting `frequency`, `damping`, `polyphony` and the model by name.
/// Assumption: `model` takes the option's name as a string in config, as
/// choice controls elsewhere in the library do.
#[test]
fn config_sets_starting_values() {
    let names = reso(json!({})).models;
    let last = names.len() - 1;
    let mut r = reso(json!({
        "model": names[last],
        "frequency": 330.0,
        "damping": 0.8,
        "polyphony": 2,
    }));
    let m = r.module.module();
    assert_eq!(m.get_control("model"), Ok(last as f32));
    assert_eq!(m.get_control("frequency"), Ok(330.0));
    assert_eq!(m.get_control("damping"), Ok(0.8));
    assert_eq!(m.get_control("polyphony"), Ok(2.0));
    r.strike();
    let x = r.render(seconds(0.5));
    assert!(
        peak(&x) > 1e-3,
        "sounds on the configured model {}",
        names[last]
    );
}

/// Criterion: every declared control accepts both ends of its declared range
/// (numbers min and max, choices every option) and the module keeps
/// producing bounded audio afterwards.
#[test]
fn every_declared_control_accepts_its_whole_range() {
    let mut r = reso(json!({}));
    for meta in r.controls.clone() {
        match meta.kind {
            ControlKind::Number { min, max, .. } => {
                for value in [min, max] {
                    r.set(&meta.key, value);
                    let got = r.module.module().get_control(&meta.key).unwrap();
                    assert!(
                        (got - value).abs() <= 1e-3 * value.abs().max(1.0),
                        "{} set to {value} reads {got}",
                        meta.key
                    );
                }
            }
            ControlKind::String { options: Some(o) } => {
                for position in 0..o.len() {
                    r.set(&meta.key, position as f32);
                }
            }
            _ => {}
        }
        r.strike();
        let x = r.render(BLOCK * 2);
        assert!(
            x.iter().all(|s| s.is_finite() && s.abs() <= 1.0),
            "{} range",
            meta.key
        );
    }
}

/// Criterion (INTERFACE.md): `level` scales the output and is smoothed, so
/// `level` 0 is silent once the glide has finished: below -80 dBFS on
/// `audio`, `odd` and `even` for a struck voice of every model.
/// Assumption: 0.3 s is long enough for the glide to finish.
#[test]
fn level_zero_is_silent() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        let mut r = ready(position, 220.0, 0.9);
        r.set("level", 0.0);
        r.render(seconds(0.3));
        r.strike();
        let out = r.render_all(seconds(1.0), BLOCK);
        for (name, x) in out.each() {
            assert!(
                peak(x) < 1e-4,
                "{model}: {name} peaks at {} with level 0",
                peak(x)
            );
        }
    }
}

/// Criterion: "unconnected `audio`: internal exciter on strum; connected
/// `audio`: internal exciter off": a strum with `audio` connected and silent
/// produces (almost) no sound, while the same strum with `audio` unconnected
/// does.
#[test]
fn connecting_audio_turns_the_internal_exciter_off() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        let mut open = ready(position, 220.0, 0.5);
        open.strike();
        assert!(
            peak(&open.render(seconds(0.5))) > 1e-3,
            "{model}: internal exciter silent"
        );

        let mut patched = ready(position, 220.0, 0.5);
        patched.connect_audio();
        patched.render(BLOCK);
        patched.strike();
        let x = patched.render(seconds(0.5));
        assert!(
            peak(&x) < 1e-4,
            "{model}: strum sounds with audio connected ({})",
            peak(&x)
        );
    }
}

/// Criterion (project definition of done): "an example invention under
/// `examples/` uses `resonator`": some `examples/*.json` has a module of that
/// type, whose config builds and which loads as an invention.
#[test]
fn an_example_invention_uses_the_resonator() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
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
            if module["type"] == "resonator" {
                let config = module.get("config").cloned().unwrap_or_else(|| json!({}));
                try_build(&config)
                    .unwrap_or_else(|err| panic!("{}: config {config}: {err}", path.display()));
                found.push(path.clone());
            }
        }
    }
    assert!(!found.is_empty(), "no examples/*.json uses resonator");
    for path in found {
        fugue::Invention::from_file(path.to_str().unwrap())
            .unwrap_or_else(|err| panic!("{} loads as an invention: {err}", path.display()));
    }
}
