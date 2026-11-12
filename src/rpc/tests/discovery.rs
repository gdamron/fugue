use super::*;
use crate::ModuleRegistry;

#[test]
fn full_discovery_exposes_metadata_or_an_explicit_error() {
    let registry = ModuleRegistry::default();
    let catalog = ModuleTypeList::from_registry(
        &registry,
        44_100,
        RegistryScope::Builtins,
        &ModuleTypeQuery {
            types: Some(vec!["oscillator".into(), "audio_file_sink".into()]),
            detail: TypeDetail::Full,
        },
    )
    .unwrap();
    let entries = catalog.details.unwrap();
    assert!(
        matches!(&entries[0], ModuleTypeDetail::Unavailable { type_name, error } if type_name == "audio_file_sink" && error.code == RpcErrorCode::ModuleBuildFailed)
    );
    match &entries[1] {
        ModuleTypeDetail::Available { info } => {
            assert_eq!(info.type_name, "oscillator");
            assert!(info.outputs.contains(&"audio".into()));
            assert!(info.controls.iter().any(|c| c.key == "frequency"));
        }
        _ => panic!("oscillator defaults should be inspectable"),
    }
    let json = serde_json::to_value(&entries[0]).unwrap();
    assert!(json.get("controls").is_none());
    assert!(json.get("inputs").is_none());
}

fn full_detail(types: &[&str]) -> Vec<ModuleTypeInfo> {
    let catalog = ModuleTypeList::from_registry(
        &ModuleRegistry::default(),
        44_100,
        RegistryScope::Builtins,
        &ModuleTypeQuery {
            types: Some(types.iter().map(|name| name.to_string()).collect()),
            detail: TypeDetail::Full,
        },
    )
    .unwrap();
    (catalog.details.unwrap().into_iter())
        .map(|entry| match entry {
            ModuleTypeDetail::Available { info } => info,
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn full_discovery_lists_the_labels_a_type_declares_where_they_say_more() {
    let infos = full_detail(&["clock", "filter", "oscillator"]);
    let (clock, filter, oscillator) = (&infos[0], &infos[1], &infos[2]);
    let declares = |info: &ModuleTypeInfo| {
        (info.controls.iter()).any(|c| c.label.is_some() || !c.option_labels.is_empty())
    };
    assert!(declares(clock) && declares(filter) && declares(oscillator));
    let bpm = clock.controls.iter().find(|c| c.key == "bpm").unwrap();
    assert_eq!(bpm.label.as_deref(), Some("Tempo"));
    assert_eq!(oscillator.input_labels["frequency_mod"], "FM");
    assert_eq!(oscillator.input_labels["amplitude_mod"], "AM");
    assert!(!oscillator.input_labels.contains_key("frequency"));
    assert!(oscillator.output_labels.is_empty() && oscillator.label.is_none());

    let control = |info: &ModuleTypeInfo, key: &str| {
        (info.controls.iter())
            .find(|control| control.key == key)
            .cloned()
            .unwrap()
    };
    let depth = control(oscillator, "frequency_mod_depth");
    assert_eq!(depth.label.as_deref(), Some("FM depth"));
    assert_eq!(depth.widget, Some(crate::ControlWidget::Knob));
    let types = control(filter, "filter_type");
    assert_eq!(types.option_labels["lowpass"], "Low-pass");
    assert!(!types.option_labels.contains_key("bell"));
    assert_eq!(types.widget, Some(crate::ControlWidget::Menu));
    assert_eq!(control(filter, "cutoff").label, None);

    let wire = serde_json::to_value(filter).unwrap();
    assert!(wire.get("label").is_none() && wire.get("input_labels").is_none());
    let bypass = &wire["controls"].as_array().unwrap()[filter
        .controls
        .iter()
        .position(|c| c.key == "bypass")
        .unwrap()];
    for field in ["label", "option_labels", "widget"] {
        assert!(bypass.get(field).is_none(), "{field}: {bypass}");
    }
}

#[test]
fn no_builtin_type_declares_a_blank_label() {
    let registry = ModuleRegistry::default();
    for type_name in registry.types() {
        let labels = registry.labels(type_name);
        let all = (labels.module.into_iter()).chain(
            labels
                .inputs
                .iter()
                .chain(labels.outputs)
                .map(|(_, label)| *label),
        );
        for label in all {
            assert!(
                !label.trim().is_empty(),
                "{type_name} declares a blank label"
            );
        }
    }
}

#[test]
fn control_metadata_from_before_labels_still_reads_and_an_unknown_editor_is_refused() {
    let older = serde_json::json!({
        "key": "cutoff",
        "description": "Cutoff frequency in Hz",
        "default": 1000.0,
        "kind": { "Number": { "min": 20.0, "max": 20000.0 } }
    });
    let meta: crate::ControlMeta = serde_json::from_value(older.clone()).unwrap();
    assert_eq!(
        (meta.label, meta.option_labels.len(), meta.widget),
        (None, 0, None)
    );
    let mut labeled = older;
    labeled["widget"] = serde_json::json!("dial");
    assert!(serde_json::from_value::<crate::ControlMeta>(labeled).is_err());
}
