//! Reload compares a declared numeric key by value, so `440` and `440.0`
//! are no change, while an integer key stays exact and an undeclared key
//! compares as JSON.

use serde_json::{json, Value};

use super::*;
use crate::ControlWriteIntent;

/// Plans reloading `module_type` from config `previous` to `new` with the
/// built-in registry's declared keys, every key a control.
fn plan(module_type: &str, previous: Value, new: Value) -> ReloadPlan {
    let registry = ModuleRegistry::default();
    let current: IndexMap<String, RuntimeModuleInfo> = [(
        "m".to_string(),
        RuntimeModuleInfo {
            id: "m".to_string(),
            module_type: module_type.to_string(),
            config: previous,
        },
    )]
    .into_iter()
    .collect();
    let new = Invention {
        modules: vec![ModuleSpec {
            id: "m".to_string(),
            module_type: module_type.to_string(),
            config: new,
        }],
        ..doc(r#"{ "modules": [], "connections": [] }"#)
    };
    plan_reload(
        &current,
        &[],
        &new,
        &HashSet::new(),
        |module_type| {
            let keys = registry.config_keys(module_type);
            (keys, registry.reload_keys(module_type))
        },
        |_, _| None,
        |_, _| true,
    )
    .unwrap()
}

/// The control writes a plan makes, as `key = value`.
fn writes(plan: &ReloadPlan) -> Vec<String> {
    assert!(plan.swapped.is_empty(), "{plan:?}");
    plan.control_updates
        .iter()
        .map(|(_, key, value)| format!("{key} = {value:?}"))
        .collect()
}

#[test]
fn reloading_a_files_float_spelling_against_a_runtime_built_from_the_integer_writes_nothing() {
    let mut running = start(&BASE.replace("440.0", "440"));

    let report = running.reload(doc(BASE)).expect("diff applies");

    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(report.unchanged, 3, "{report:?}");
}

#[test]
fn reloading_the_original_keeps_a_value_performed_after_authoring_the_files_value_back() {
    // FUG-289's round-3 case. The file has 440.0; authoring 330 then 440
    // leaves the stored config holding the integer 440. The performed 300
    // must survive a reload of the file, which still says 440.
    let mut running = start(BASE);
    for value in [330.0, 440.0] {
        running
            .set_control("osc1", "frequency", ControlValue::Number(value))
            .unwrap();
    }
    running
        .snapshot()
        .set_control_with_intent(
            "osc1",
            "frequency",
            ControlValue::Number(300.0),
            ControlWriteIntent::Perform,
        )
        .unwrap();

    let report = running.reload(doc(BASE)).expect("diff applies");

    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        ControlValue::Number(300.0)
    );
}

#[test]
fn a_float_key_compares_the_f32_its_reader_reads() {
    let frequency = |value: Value| json!({ "frequency": value });
    assert!(writes(&plan(
        "oscillator",
        frequency(json!(440)),
        frequency(json!(440.0))
    ))
    .is_empty());
    assert!(writes(&plan(
        "oscillator",
        frequency(json!(261.63)),
        frequency(json!(261.6300048828125))
    ))
    .is_empty());
    assert_eq!(
        writes(&plan(
            "oscillator",
            frequency(json!(440)),
            frequency(json!(440.5))
        )),
        ["frequency = Number(440.5)"]
    );
}

#[test]
fn root_note_72_and_72_point_0_are_one_note_and_73_is_another() {
    let root = |value: Value| json!({ "root_note": value });
    assert!(writes(&plan("melody", root(json!(72)), root(json!(72.0)))).is_empty());
    assert!(writes(&plan("melody", root(json!(72.0)), root(json!(72)))).is_empty());
    assert_eq!(
        writes(&plan("melody", root(json!(72.0)), root(json!(73)))),
        ["root_note = Number(73.0)"]
    );
    // A fractional note is no integer the reader takes: it is a change,
    // never the same note.
    assert_eq!(
        writes(&plan("melody", root(json!(72)), root(json!(72.5)))),
        ["root_note = Number(72.5)"]
    );
}

#[test]
fn an_integer_only_count_compares_whole_values() {
    let channels = |value: Value| json!({ "channel_count": value });
    assert!(writes(&plan("mixer", channels(json!(4)), channels(json!(4.0)))).is_empty());
    assert_eq!(
        writes(&plan("mixer", channels(json!(4)), channels(json!(5.0)))),
        ["channel_count = Number(5.0)"]
    );
}

#[test]
fn a_seed_past_2_pow_53_compares_exactly_and_never_equals_a_float() {
    let seed = |value: Value| json!({ "seed": value });
    let past = 9_007_199_254_740_993_u64; // 2^53 + 1
    assert!(writes(&plan("melody", seed(json!(past)), seed(json!(past)))).is_empty());
    // As f64 these two are one number; as integers they are two seeds.
    assert_eq!(
        plan("melody", seed(json!(past)), seed(json!(past - 1)))
            .control_updates
            .len(),
        1
    );
    // A float this large is refused by the reader, so it is a change even
    // where it holds the integer exactly.
    let big = 1_u64 << 60;
    assert_eq!(
        plan("melody", seed(json!(big)), seed(json!(big as f64)))
            .control_updates
            .len(),
        1
    );
}

#[test]
fn an_undeclared_key_compares_as_json() {
    // Mixer `levels` elements and an oscillator's undeclared keys are not
    // declared numeric keys, so their spelling still counts.
    let plan = plan(
        "mixer",
        json!({ "channel_count": 2, "levels": [1, 1] }),
        json!({ "channel_count": 2.0, "levels": [1.0, 1] }),
    );
    assert_eq!(plan.swapped.len(), 1, "{plan:?}");
}

/// A development exposing its oscillator's frequency as `freq`, played at
/// `freq` from the file.
fn voiced(freq: &str) -> String {
    format!(
        r#"{{
        "version": "1.0.0",
        "developments": [{{
            "name": "voice",
            "definition": {{
                "modules": [{{ "id": "o", "type": "oscillator" }}],
                "connections": [],
                "outputs": [{{ "name": "audio", "from": "o", "from_port": "audio" }}],
                "controls": [{{ "key": "freq", "module": "o", "control": "frequency" }}]
            }}
        }}],
        "modules": [
            {{ "id": "v", "type": "voice", "config": {{ "freq": {freq} }} }},
            {{ "id": "dac", "type": "dac" }}
        ],
        "connections": [{{ "from": "v", "from_port": "audio", "to": "dac", "to_port": "audio" }}]
    }}"#
    )
}

#[test]
fn a_declared_control_compares_by_value_even_where_its_type_declares_no_config_key() {
    // A development's config keys are its exposed controls, which its type
    // does not list as config keys: the running control's kind reads them.
    let mut running = start(&voiced("440"));
    running
        .snapshot()
        .set_control_with_intent(
            "v",
            "freq",
            ControlValue::Number(300.0),
            ControlWriteIntent::Perform,
        )
        .unwrap();

    let report = running.reload(doc(&voiced("440.0"))).expect("diff applies");

    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert!(report.swapped.is_empty(), "{report:?}");
    assert_eq!(
        running.get_control("v", "freq").unwrap(),
        ControlValue::Number(300.0),
        "the performed value stands"
    );

    let report = running.reload(doc(&voiced("220"))).expect("diff applies");
    assert_eq!(report.controls_updated, ["v.freq"]);
}
