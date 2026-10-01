use std::collections::HashMap;

use serde_json::json;

use super::*;
use crate::rpc::{EditFailureReason, EditOp};
use crate::ControlValue;

mod runtime_equivalence;

const BASE: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc1", "type": "oscillator", "config": { "frequency": 440 } },
        { "id": "osc2", "type": "oscillator", "config": { "frequency": 550 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "osc2", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

fn base() -> Invention {
    Invention::from_json(BASE).unwrap()
}

fn type_facts(module_type: &str) -> Option<ModuleFacts> {
    let names = |names: &[&str]| names.iter().map(|name| name.to_string()).collect();
    let number = || ControlKind::Number {
        min: 0.0,
        max: 20_000.0,
    };
    match module_type {
        "oscillator" => Some(ModuleFacts {
            inputs: names(&["frequency", "fm"]),
            outputs: names(&["audio"]),
            controls: BTreeMap::from([
                ("frequency".to_string(), number()),
                (
                    "type".to_string(),
                    ControlKind::String {
                        options: Some(vec!["sine".into(), "square".into()]),
                    },
                ),
            ]),
        }),
        "lfo" => Some(ModuleFacts {
            inputs: names(&["rate"]),
            outputs: names(&["out"]),
            controls: BTreeMap::from([
                ("frequency".to_string(), number()),
                ("retrigger".to_string(), ControlKind::Bool),
            ]),
        }),
        "dac" => Some(ModuleFacts {
            inputs: names(&["audio"]),
            ..ModuleFacts::default()
        }),
        _ => None,
    }
}

/// Runtime facts derived from the base document's types; a config holding
/// `"broken": true` stands in for a config the type refuses.
struct FakeFacts {
    running: HashMap<String, ModuleFacts>,
    described: Vec<String>,
}

impl FakeFacts {
    fn for_document(document: &Invention) -> Self {
        Self {
            running: document
                .modules
                .iter()
                .map(|spec| (spec.id.clone(), type_facts(&spec.module_type).unwrap()))
                .collect(),
            described: Vec::new(),
        }
    }
}

impl EditFacts for FakeFacts {
    fn module(&self, id: &str) -> Option<ModuleFacts> {
        self.running.get(id).cloned()
    }

    fn has_type(&self, module_type: &str) -> bool {
        type_facts(module_type).is_some()
    }

    fn describe(
        &mut self,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<ModuleFacts, String> {
        self.described.push(id.to_string());
        if config.get("broken").is_some() {
            return Err("broken config".to_string());
        }
        Ok(type_facts(module_type).unwrap())
    }
}

fn apply(edits: Vec<StructuralEdit>) -> Result<EditedCandidate, EditFailure> {
    let document = base();
    let mut facts = FakeFacts::for_document(&document);
    apply_to_candidate(&document, &edits, &mut facts)
}

fn refused(edits: Vec<StructuralEdit>) -> EditFailure {
    apply(edits).expect_err("the batch should be refused")
}

fn add(id: &str, module_type: &str, config: serde_json::Value) -> StructuralEdit {
    StructuralEdit::AddModule {
        id: id.into(),
        module_type: module_type.into(),
        config,
    }
}

fn remove(id: &str) -> StructuralEdit {
    StructuralEdit::RemoveModule { id: id.into() }
}

fn connect(from: &str, from_port: &str, to: &str, to_port: &str) -> StructuralEdit {
    StructuralEdit::Connect {
        from: from.into(),
        from_port: from_port.into(),
        to: to.into(),
        to_port: to_port.into(),
    }
}

fn disconnect(from: &str, from_port: &str, to: &str, to_port: &str) -> StructuralEdit {
    StructuralEdit::Disconnect {
        from: from.into(),
        from_port: from_port.into(),
        to: to.into(),
        to_port: to_port.into(),
    }
}

fn set(module_id: &str, key: &str, value: ControlValue) -> StructuralEdit {
    StructuralEdit::SetControl {
        module_id: module_id.into(),
        key: key.into(),
        value,
    }
}

fn config_of<'d>(document: &'d Invention, id: &str) -> &'d serde_json::Value {
    &document
        .modules
        .iter()
        .find(|spec| spec.id == id)
        .unwrap()
        .config
}

fn has_connection(document: &Invention, from: &str, to: &str, to_port: &str) -> bool {
    document
        .connections
        .iter()
        .any(|conn| conn.from == from && conn.to == to && conn.to_port.as_deref() == Some(to_port))
}

#[test]
fn later_edits_see_earlier_ones() {
    let candidate = apply(vec![
        add("lfo", "lfo", json!({ "frequency": 2 })),
        connect("lfo", "out", "osc1", "fm"),
        set("lfo", "retrigger", ControlValue::Bool(true)),
    ])
    .unwrap();
    assert_eq!(candidate.document.modules.last().unwrap().id, "lfo");
    assert_eq!(
        config_of(&candidate.document, "lfo"),
        &json!({ "frequency": 2, "retrigger": true })
    );
    assert!(has_connection(&candidate.document, "lfo", "osc1", "fm"));
    assert_eq!(
        candidate.named_modules,
        BTreeSet::from(["lfo".to_string(), "osc1".to_string()])
    );
}

#[test]
fn the_source_document_is_never_changed() {
    let document = base();
    let mut facts = FakeFacts::for_document(&document);
    let edits = vec![remove("osc1"), connect("osc1", "audio", "dac", "audio")];
    assert!(apply_to_candidate(&document, &edits, &mut facts).is_err());
    assert_eq!(document, base());
}

#[test]
fn referencing_a_removed_module_fails_at_its_index() {
    let failure = refused(vec![
        set("osc2", "frequency", ControlValue::Number(1.0)),
        remove("osc1"),
        connect("osc1", "audio", "dac", "audio"),
    ]);
    assert_eq!(failure.index, 2);
    assert_eq!(failure.op, EditOp::Connect);
    assert_eq!(failure.reason, EditFailureReason::UnknownModule);
    assert!(failure.message.contains("osc1"));

    let failure = refused(vec![remove("osc1"), remove("osc1")]);
    assert_eq!(
        (failure.index, failure.reason),
        (1, EditFailureReason::UnknownModule)
    );
}

#[test]
fn adding_an_existing_id_is_refused() {
    let failure = refused(vec![add("osc1", "oscillator", json!(null))]);
    assert_eq!(
        (failure.index, failure.reason),
        (0, EditFailureReason::DuplicateModule)
    );

    let failure = refused(vec![
        add("lfo", "lfo", json!(null)),
        add("lfo", "lfo", json!(null)),
    ]);
    assert_eq!(
        (failure.index, failure.reason),
        (1, EditFailureReason::DuplicateModule)
    );
}

#[test]
fn removing_then_adding_an_id_replaces_it_with_the_new_type() {
    let candidate = apply(vec![
        remove("osc1"),
        add("osc1", "lfo", json!(null)),
        connect("osc1", "out", "osc2", "fm"),
        set("osc1", "retrigger", ControlValue::Bool(false)),
    ])
    .unwrap();
    let spec = candidate
        .document
        .modules
        .iter()
        .find(|spec| spec.id == "osc1")
        .unwrap();
    assert_eq!(spec.module_type, "lfo");
    assert!(has_connection(&candidate.document, "osc1", "osc2", "fm"));
}

#[test]
fn unknown_types_and_refused_configs_name_the_add() {
    let failure = refused(vec![add("x", "theremin", json!(null))]);
    assert_eq!(failure.reason, EditFailureReason::UnknownModuleType);

    let document = base();
    let mut facts = FakeFacts::for_document(&document);
    let failure = apply_to_candidate(
        &document,
        &[
            add("ok", "lfo", json!(null)),
            add("bad", "lfo", json!({ "broken": true })),
        ],
        &mut facts,
    )
    .unwrap_err();
    assert_eq!(
        (failure.index, failure.reason),
        (1, EditFailureReason::InvalidConfig)
    );
    assert!(failure.message.contains("broken config"));
    assert_eq!(facts.described, ["ok", "bad"]);
}

#[test]
fn connections_must_name_existing_ports_once() {
    let failure = refused(vec![connect("osc1", "audio", "osc2", "phase")]);
    assert_eq!(failure.reason, EditFailureReason::UnknownPort);
    assert!(failure.message.contains("frequency, fm"));

    let failure = refused(vec![connect("dac", "audio", "osc2", "fm")]);
    assert_eq!(failure.reason, EditFailureReason::UnknownPort);
    assert!(failure.message.contains("available: none"));

    let failure = refused(vec![connect("osc1", "audio", "dac", "audio")]);
    assert_eq!(failure.reason, EditFailureReason::ConnectionExists);
}

#[test]
fn disconnecting_a_missing_connection_is_refused() {
    let failure = refused(vec![disconnect("osc1", "audio", "osc2", "fm")]);
    assert_eq!(failure.reason, EditFailureReason::ConnectionNotFound);

    let failure = refused(vec![
        disconnect("osc1", "audio", "dac", "audio"),
        disconnect("osc1", "audio", "dac", "audio"),
    ]);
    assert_eq!(
        (failure.index, failure.reason),
        (1, EditFailureReason::ConnectionNotFound)
    );

    let failure = refused(vec![disconnect("ghost", "audio", "dac", "audio")]);
    assert_eq!(failure.reason, EditFailureReason::UnknownModule);
}

#[test]
fn removing_a_module_drops_its_connections() {
    let candidate = apply(vec![connect("osc1", "audio", "osc2", "fm"), remove("osc1")]).unwrap();
    assert!(candidate
        .document
        .connections
        .iter()
        .all(|conn| conn.from != "osc1" && conn.to != "osc1"));
    assert!(has_connection(&candidate.document, "osc2", "dac", "audio"));
}

#[test]
fn unknown_controls_are_refused() {
    let failure = refused(vec![set("osc1", "cutoff", ControlValue::Number(1.0))]);
    assert_eq!(failure.reason, EditFailureReason::UnknownControl);
    assert!(failure.message.contains("frequency, type"));

    let failure = refused(vec![set("dac", "level", ControlValue::Number(1.0))]);
    assert_eq!(failure.reason, EditFailureReason::UnknownControl);
}

#[test]
fn control_values_are_coerced_to_the_declared_kind() {
    let candidate = apply(vec![
        set("osc1", "frequency", ControlValue::String("330".into())),
        set("osc2", "frequency", ControlValue::Number(0.7)),
        set("osc2", "type", ControlValue::String("square".into())),
        add("lfo", "lfo", json!(null)),
        set("lfo", "retrigger", ControlValue::String("true".into())),
    ])
    .unwrap();
    // Written as the runtime writes them: integral numbers stay integers,
    // fractions keep their shortest decimal form.
    assert_eq!(
        config_of(&candidate.document, "osc1"),
        &json!({ "frequency": 330 })
    );
    assert_eq!(
        config_of(&candidate.document, "osc2"),
        &json!({ "frequency": 0.7, "type": "square" })
    );
    assert_eq!(
        config_of(&candidate.document, "lfo"),
        &json!({ "retrigger": true })
    );
    let applied: Vec<(String, ControlValue)> = candidate
        .control_writes
        .iter()
        .map(|write| {
            (
                format!("{}.{}", write.module_id, write.key),
                write.value.clone(),
            )
        })
        .collect();
    assert_eq!(
        applied,
        [
            ("osc1.frequency".into(), ControlValue::Number(330.0)),
            ("osc2.frequency".into(), ControlValue::Number(0.7)),
            ("osc2.type".into(), ControlValue::String("square".into())),
            ("lfo.retrigger".into(), ControlValue::Bool(true)),
        ]
    );
    assert!(candidate
        .control_writes
        .iter()
        .all(|write| write.intent.is_authoring()));

    let failure = refused(vec![set(
        "osc1",
        "frequency",
        ControlValue::String("loud".into()),
    )]);
    assert_eq!(failure.reason, EditFailureReason::InvalidControlValue);
    let failure = refused(vec![set("osc1", "frequency", ControlValue::Bool(true))]);
    assert_eq!(failure.reason, EditFailureReason::InvalidControlValue);
}

#[test]
fn names_are_checked_in_batch_order() {
    let failure = refused(vec![
        set("osc1", "frequency", ControlValue::Number(1.0)),
        remove(""),
        remove("ghost"),
    ]);
    assert_eq!(
        (failure.index, failure.op, failure.reason),
        (1, EditOp::RemoveModule, EditFailureReason::InvalidName)
    );
}

#[test]
fn a_module_in_the_document_but_not_running_is_unknown() {
    let document = base();
    let mut facts = FakeFacts::for_document(&document);
    facts.running.remove("osc2");
    let failure = apply_to_candidate(
        &document,
        &[connect("osc2", "audio", "osc1", "fm")],
        &mut facts,
    )
    .unwrap_err();
    assert_eq!(failure.reason, EditFailureReason::UnknownModule);
    assert!(failure.message.contains("not running"));
}
