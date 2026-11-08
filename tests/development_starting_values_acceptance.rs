//! Acceptance tests for a development instance starting at its configured
//! values.
//!
//! The `config` of an instance of a development sets the development's
//! exposed controls. Those values are built into the inner modules before
//! the first audio block, so they do not glide in from the inner modules'
//! defaults. A value the exposed control refuses (a string for a numeric
//! control, a fraction for an integer one) is reported as a warning and
//! skipped: the load still succeeds and the other values still start in
//! place. A value a live write would coerce (clamp) starts at the coerced
//! value. A config of only valid values renders exactly as the same values
//! written into the inner modules' own config.
//!
//! The development is `voice`, the instance `lead`. A still square wave
//! (a constant 1.0 for the first seconds) runs through its modules, so the
//! left channel of every frame is the product of the levels started, and a
//! glide from a default shows up as frames that differ from that product.

use fugue::{ControlValue, RenderEngine};
use serde_json::{json, Value};

const RATE: u32 = 48_000;
/// Frames checked from the first sample on: more than a few blocks.
const FRAMES: usize = 2_048;

// -- documents ------------------------------------------------------------------

fn vca(id: &str) -> Value {
    json!({ "id": id, "type": "vca" })
}

fn gain(id: &str) -> Value {
    json!({ "id": id, "type": "gain" })
}

/// A development `voice` passing a still square wave through `chain`, each
/// entry `(module, input port, output port)`, with `controls` exposed; one
/// instance `lead` configured with `config`, into the dac.
fn voice(chain: &[(Value, &str, &str)], controls: Value, config: Value) -> Value {
    let mut modules = vec![json!({
        "id": "osc", "type": "oscillator",
        "config": { "frequency": 0.001, "waveform": "square" }
    })];
    let mut connections = Vec::new();
    let mut from = ("osc".to_string(), "audio");
    for (module, input, output) in chain {
        let id = module["id"].as_str().unwrap().to_string();
        connections.push(json!({
            "from": from.0, "from_port": from.1, "to": id, "to_port": input
        }));
        modules.push(module.clone());
        from = (id, output);
    }
    json!({
        "version": "1.0.0",
        "developments": [{ "name": "voice", "definition": {
            "version": "1.0.0",
            "modules": modules,
            "connections": connections,
            "outputs": [{ "name": "audio", "from": from.0, "from_port": from.1 }],
            "controls": controls
        }}],
        "modules": [
            { "id": "lead", "type": "voice", "config": config },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [{ "from": "lead", "from_port": "audio", "to": "dac", "to_port": "audio" }]
    })
}

fn exposed(name: &str, module: &str, control: &str) -> Value {
    json!({ "name": name, "module": module, "control": control })
}

/// `voice` with a vca `vca` and a gain `gain`, exposed as `level` and
/// `decibels`, and a resonator `res` outside the signal path whose integer
/// `polyphony` is exposed too.
fn level_decibels_polyphony(config: Value) -> Value {
    let mut doc = voice(
        &[
            (vca("vca"), "audio", "audio"),
            (gain("gain"), "audio", "audio"),
        ],
        json!([
            exposed("level", "vca", "level"),
            exposed("decibels", "gain", "decibels"),
            exposed("polyphony", "res", "polyphony"),
        ]),
        config,
    );
    doc["developments"][0]["definition"]["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "res", "type": "resonator" }));
    doc
}

/// `voice` with vcas `vca` and `trim` and a gain `gain`, exposed as `level`,
/// `trim` and `decibels`.
fn level_trim_decibels(config: Value) -> Value {
    voice(
        &[
            (vca("vca"), "audio", "audio"),
            (vca("trim"), "audio", "audio"),
            (gain("gain"), "audio", "audio"),
        ],
        json!([
            exposed("level", "vca", "level"),
            exposed("trim", "trim", "level"),
            exposed("decibels", "gain", "decibels"),
        ]),
        config,
    )
}

// -- rendering ------------------------------------------------------------------

/// The document loaded; the load must succeed.
fn load(doc: &Value) -> RenderEngine {
    let mut engine = RenderEngine::new(RATE);
    if let Err(e) = engine.load_json(&doc.to_string()) {
        panic!("the load failed: {e}\n{doc:#}");
    }
    engine
}

/// The first `frames` frames, interleaved stereo, from the first sample.
fn render(engine: &mut RenderEngine, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames * 2];
    engine.render_interleaved(&mut out).unwrap();
    out
}

fn left(interleaved: &[f32]) -> Vec<f32> {
    interleaved.chunks(2).map(|frame| frame[0]).collect()
}

/// Every frame of `out` is `expected`, to within rounding.
fn assert_steady(out: &[f32], expected: f32) {
    for (i, &x) in out.iter().enumerate() {
        assert!(
            (x - expected).abs() < 1e-5,
            "frame {i}: {x}, not {expected}"
        );
    }
}

fn level_of(decibels: f32) -> f32 {
    10f32.powf(decibels / 20.0)
}

/// One warning names the development, the instance and the control.
fn assert_names(warnings: &[String], control: &str) {
    assert_eq!(
        warnings.len(),
        1,
        "one refused value, one warning: {warnings:?}"
    );
    let text = &warnings[0];
    for name in ["voice", "lead", control] {
        assert!(text.contains(name), "the warning lacks `{name}`: {text}");
    }
}

fn number(engine: &RenderEngine, module: &str, key: &str) -> f32 {
    match engine.get_control(module, key).unwrap() {
        ControlValue::Number(n) => n,
        other => panic!("{module}.{key} is not a number: {other:?}"),
    }
}

// -- refused values ---------------------------------------------------------------

/// A value its control refuses (a string for a numeric control) leaves the
/// two valid values in place from the first sample, whichever control it is.
#[test]
fn the_valid_values_start_in_place_beside_a_refused_one() {
    for refused in ["level", "trim", "decibels"] {
        let mut config = json!({ "level": 0.25, "trim": 0.5, "decibels": -12.0 });
        config[refused] = json!("loud");
        let mut engine = load(&level_trim_decibels(config));

        let mut expected = 1.0;
        if refused != "level" {
            expected *= 0.25;
        }
        if refused != "trim" {
            expected *= 0.5;
        }
        if refused != "decibels" {
            expected *= level_of(-12.0);
        }
        let out = left(&render(&mut engine, FRAMES));
        assert_steady(&out, expected);
    }
}

/// The load reports the refused value as a warning naming the development,
/// the instance and the control, and still succeeds.
#[test]
fn a_refused_value_is_reported_by_development_instance_and_control() {
    for refused in ["level", "trim", "decibels"] {
        let mut config = json!({ "level": 0.25, "trim": 0.5, "decibels": -12.0 });
        config[refused] = json!("loud");
        let engine = load(&level_trim_decibels(config));
        assert_names(&engine.load_warnings(), refused);
    }
}

/// A config of only valid values reports no warning.
#[test]
fn a_config_of_valid_values_reports_no_warning() {
    let engine = load(&level_trim_decibels(
        json!({ "level": 0.25, "trim": 0.5, "decibels": -12.0 }),
    ));
    assert!(engine.load_warnings().is_empty());
    let none = load(&level_trim_decibels(json!({})));
    assert!(none.load_warnings().is_empty());
}

/// The warnings are those of the last load: a clean load after a refused
/// one reports none.
#[test]
fn the_warnings_are_those_of_the_last_load() {
    let mut engine = load(&level_trim_decibels(json!({ "level": "loud" })));
    assert_eq!(engine.load_warnings().len(), 1);
    let clean = level_trim_decibels(json!({ "level": 0.5 }));
    engine.load_json(&clean.to_string()).unwrap();
    assert!(engine.load_warnings().is_empty());
}

/// An integer control given a fraction (2.6) is refused by a live write, so
/// it is a refused starting value: warned, the control keeps its default,
/// and the other values start in place.
#[test]
fn a_fraction_for_an_integer_control_is_refused_and_the_rest_start_in_place() {
    let config = json!({ "level": 0.25, "decibels": -12.0, "polyphony": 2.6 });
    let mut engine = load(&level_decibels_polyphony(config));
    assert_names(&engine.load_warnings(), "polyphony");
    assert_eq!(number(&engine, "lead", "polyphony"), 1.0);
    assert_steady(&left(&render(&mut engine, FRAMES)), 0.25 * level_of(-12.0));
}

/// A string for the integer control is refused the same way, beside values
/// that start in place.
#[test]
fn a_string_for_an_integer_control_is_refused_and_the_rest_start_in_place() {
    let config = json!({ "level": 0.25, "decibels": -12.0, "polyphony": "many" });
    let mut engine = load(&level_decibels_polyphony(config));
    assert_names(&engine.load_warnings(), "polyphony");
    assert_steady(&left(&render(&mut engine, FRAMES)), 0.25 * level_of(-12.0));
}

/// A refused value on a control that fans out to two inner modules leaves
/// both at their defaults, is reported, and the valid values start in place.
#[test]
fn a_refused_value_on_a_fan_out_control_is_reported_and_the_rest_start() {
    let fan_out = |config: Value| {
        voice(
            &[
                (vca("a"), "audio", "audio"),
                (vca("b"), "audio", "audio"),
                (gain("gain"), "audio", "audio"),
            ],
            json!([
                exposed("level", "a", "level"),
                exposed("level", "b", "level"),
                exposed("decibels", "gain", "decibels"),
            ]),
            config,
        )
    };

    let mut engine = load(&fan_out(json!({ "level": "loud", "decibels": -12.0 })));
    let warnings = engine.load_warnings();
    assert!(!warnings.is_empty(), "the refused value is reported");
    for name in ["voice", "lead", "level"] {
        assert!(
            warnings.iter().any(|w| w.contains(name)),
            "no warning names `{name}`: {warnings:?}"
        );
    }
    assert_steady(&left(&render(&mut engine, FRAMES)), level_of(-12.0));

    // Valid, both targets start at the value: 0.5 * 0.5 through the chain.
    let mut engine = load(&fan_out(json!({ "level": 0.5, "decibels": -12.0 })));
    assert!(engine.load_warnings().is_empty());
    assert_steady(&left(&render(&mut engine, FRAMES)), 0.25 * level_of(-12.0));
}

/// The outer instance's config passes a refused value down to an inner
/// development: the load succeeds, the value is reported with its control's
/// name, and the outer instance's other value starts in place.
#[test]
fn a_refused_value_passed_down_to_a_nested_development_is_reported() {
    let mut doc = voice(
        &[
            (json!({ "id": "nest", "type": "inner" }), "audio", "audio"),
            (gain("gain"), "audio", "audio"),
        ],
        json!([
            exposed("level", "nest", "level"),
            exposed("decibels", "gain", "decibels"),
        ]),
        json!({ "level": "loud", "decibels": -12.0 }),
    );
    doc["developments"][0]["definition"]["developments"] = json!([{
        "name": "inner",
        "definition": {
            "version": "1.0.0",
            "modules": [vca("vca")],
            "connections": [],
            "inputs": [{ "name": "audio", "to": "vca", "to_port": "audio" }],
            "outputs": [{ "name": "audio", "from": "vca", "from_port": "audio" }],
            "controls": [exposed("level", "vca", "level")]
        }
    }]);
    let mut engine = load(&doc);
    let warnings = engine.load_warnings();
    assert!(!warnings.is_empty(), "the refused value is reported");
    // Reported where it is given (voice/lead) or where it is refused
    // (inner/nest); either way it names its control.
    assert!(
        warnings.iter().any(|w| w.contains("level")
            && ((w.contains("voice") && w.contains("lead"))
                || (w.contains("inner") && w.contains("nest")))),
        "{warnings:?}"
    );
    assert_steady(&left(&render(&mut engine, FRAMES)), level_of(-12.0));
}

// -- coerced values ---------------------------------------------------------------

/// A ranged control given a value outside its range starts at the value a
/// live write holds it at, from the first sample: a vca `level` of 1.5 at
/// 1.0, a gain `decibels` of 40 at its 24 dB ceiling.
#[test]
fn an_out_of_range_value_starts_at_the_clamped_value() {
    let mut engine = load(&level_decibels_polyphony(
        json!({ "level": 1.5, "decibels": 40.0 }),
    ));
    assert_steady(&left(&render(&mut engine, FRAMES)), level_of(24.0));

    let mut engine = load(&level_decibels_polyphony(
        json!({ "level": 1.5, "decibels": -12.0 }),
    ));
    assert_steady(&left(&render(&mut engine, FRAMES)), level_of(-12.0));

    let mut engine = load(&level_decibels_polyphony(
        json!({ "level": -0.5, "decibels": -12.0 }),
    ));
    assert_steady(&left(&render(&mut engine, FRAMES)), 0.0);
}

/// An integer control given a value above its range (resonator `polyphony`
/// of 9) starts at what a live write of 9 leaves it at: 4.
#[test]
fn an_out_of_range_integer_starts_at_the_value_a_live_write_holds_it_at() {
    let started = load(&level_decibels_polyphony(json!({ "polyphony": 9 })));

    let written = load(&level_decibels_polyphony(json!({})));
    written
        .set_control("lead", "polyphony", ControlValue::Number(9.0))
        .unwrap();
    assert_eq!(number(&written, "lead", "polyphony"), 4.0);
    assert_eq!(number(&started, "lead", "polyphony"), 4.0);
}

// -- valid values -----------------------------------------------------------------

/// A config of only valid values renders exactly (every sample of both
/// channels, bit for bit) what the same values written into the inner
/// modules' own config render.
#[test]
fn valid_values_render_identically_to_values_written_into_the_inner_modules() {
    let mut inner = level_trim_decibels(json!({}));
    let modules = inner["developments"][0]["definition"]["modules"]
        .as_array_mut()
        .unwrap();
    for module in modules.iter_mut() {
        match module["id"].as_str().unwrap() {
            "vca" => module["config"] = json!({ "level": 0.25 }),
            "trim" => module["config"] = json!({ "level": 0.5 }),
            "gain" => module["config"] = json!({ "decibels": -12.0 }),
            "osc" => module["config"]["waveform"] = json!("sine"),
            _ => {}
        }
    }
    modules[0]["config"]["frequency"] = json!(220.0);

    let mut configured =
        level_trim_decibels(json!({ "level": 0.25, "trim": 0.5, "decibels": -12.0 }));
    configured["developments"][0]["definition"]["modules"][0]["config"] =
        json!({ "frequency": 220.0, "waveform": "sine" });

    let mut by_config = load(&configured);
    assert!(by_config.load_warnings().is_empty());
    let mut by_inner = load(&inner);
    let a = render(&mut by_config, 4 * FRAMES);
    let b = render(&mut by_inner, 4 * FRAMES);
    assert!(a.iter().any(|x| x.abs() > 0.01), "the sine is audible");
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&a), bits(&b));
}

/// The same holds through a nested development: a value passed down starts
/// exactly as the same value written into the innermost module.
#[test]
fn valid_values_passed_to_a_nested_development_render_identically_to_inner_config() {
    let nested = |outer_config: Value, vca_config: Value| {
        let mut doc = voice(
            &[(json!({ "id": "nest", "type": "inner" }), "audio", "audio")],
            json!([exposed("level", "nest", "level")]),
            outer_config,
        );
        doc["developments"][0]["definition"]["modules"][0]["config"] =
            json!({ "frequency": 330.0, "waveform": "sine" });
        doc["developments"][0]["definition"]["developments"] = json!([{
            "name": "inner",
            "definition": {
                "version": "1.0.0",
                "modules": [{ "id": "vca", "type": "vca", "config": vca_config }],
                "connections": [],
                "inputs": [{ "name": "audio", "to": "vca", "to_port": "audio" }],
                "outputs": [{ "name": "audio", "from": "vca", "from_port": "audio" }],
                "controls": [exposed("level", "vca", "level")]
            }
        }]);
        doc
    };
    let mut passed = load(&nested(json!({ "level": 0.25 }), json!({})));
    assert!(passed.load_warnings().is_empty());
    let mut written = load(&nested(json!({}), json!({ "level": 0.25 })));
    let a = render(&mut passed, 4 * FRAMES);
    let b = render(&mut written, 4 * FRAMES);
    assert!(a.iter().any(|x| x.abs() > 0.01), "the sine is audible");
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&a), bits(&b));
}
