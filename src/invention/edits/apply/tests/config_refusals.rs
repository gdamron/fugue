//! A config number a module type refuses (`1e39` for a float key, `72.5`
//! for an integer key) is refused at every entry point, naming the module,
//! and changes nothing: a load refuses the whole document, and add_module
//! (as the daemon's RPC and a script call it) and an ApplyEdits batch leave
//! the running invention as it was.

use super::*;
use crate::module_config::tests::probe::ProbeFactory;
use crate::rpc::{EditFailureReason, RpcErrorCode};
use crate::test_support::wait_until;

const NOT_FINITE: &str = "oscillator config 'frequency' expects a finite number, got 1e39";

fn registry_with_probe() -> ModuleRegistry {
    let mut registry = ModuleRegistry::default();
    registry.register(ProbeFactory);
    registry
}

fn load(registry: ModuleRegistry, module: &str) -> Result<(), String> {
    InventionBuilder::with_registry(SAMPLE_RATE, registry)
        .build(doc(&base_with(module)))
        .map(drop)
        .map_err(|error| error.to_string())
}

#[test]
fn a_load_refuses_the_whole_document_naming_the_module() {
    let error = load(
        ModuleRegistry::default(),
        r#"{ "id": "bad", "type": "oscillator", "config": { "frequency": 1e39 } }"#,
    )
    .unwrap_err();
    assert_eq!(error, format!("module 'bad': {NOT_FINITE}"));

    let error = load(
        registry_with_probe(),
        r#"{ "id": "p", "type": "probe", "config": { "hz": 72.5 } }"#,
    )
    .unwrap_err();
    assert_eq!(
        error,
        "module 'p': probe config 'hz' expects a whole number from 0 to 65535, got 72.5"
    );

    let max = json!({ "id": "big", "type": "oscillator", "config": { "frequency": f32::MAX } });
    load(ModuleRegistry::default(), &max.to_string()).unwrap();
    let (running, _pump) = start_with(
        registry_with_probe(),
        &base_with(r#"{ "id": "p", "type": "probe", "config": { "hz": 72.0 } }"#),
    );
    assert_eq!(running.get_control("p", "frequency").unwrap(), number(72.0));
}

#[test]
fn add_module_refuses_and_changes_nothing() {
    let (running, _pump) = start(BASE);
    let before = observe(&running);
    let error = running
        .add_module("osc3", "oscillator", &json!({ "frequency": 1e39 }))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("module build failed: module 'osc3': {NOT_FINITE}")
    );
    // Replacing a module in place is refused the same way.
    let error = running
        .add_module("osc1", "oscillator", &json!({ "frequency": 1e39 }))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("module 'osc1': oscillator config"),
        "{error}"
    );
    assert_eq!(observe(&running), before);

    running
        .add_module("osc3", "oscillator", &json!({ "frequency": f32::MAX }))
        .unwrap();
}

#[test]
fn a_script_add_module_refuses_and_changes_nothing() {
    let script = "function tick() { if (globalThis.done) return; globalThis.done = true; \
        try { graph.addModule('osc3', 'oscillator', { frequency: 1e39 }); \
        graph.setControl('code1', 'last_error', 'accepted'); } \
        catch (e) { graph.setControl('code1', 'last_error', String(e)); } }";
    let code =
        json!({ "id": "code1", "type": "code", "config": { "tick_hz": 20.0, "script": script } });
    let (running, _pump) = start(&base_with(&code.to_string()));
    let before = observe(&running);
    let mut report = String::new();
    let reported = wait_until(|| {
        if let Ok(ControlValue::String(text)) = running.get_control("code1", "last_error") {
            report = text;
        }
        !report.is_empty()
    });
    assert!(reported, "the script never reported");
    assert!(
        report.contains(&format!("module 'osc3': {NOT_FINITE}")),
        "{report}"
    );
    assert_eq!(observe(&running).modules, before.modules);
    assert_eq!(observe(&running).document, before.document);
}

#[test]
fn an_edit_batch_refuses_at_the_add_and_changes_nothing() {
    let (mut running, _pump) = start_with(registry_with_probe(), BASE);
    let refusals = [
        (
            vec![add("osc3", "oscillator", json!({ "frequency": 1e39 }))],
            0,
        ),
        // An upsert: the module is removed and added again in one batch.
        (
            vec![
                remove("osc1"),
                add("osc1", "oscillator", json!({ "frequency_mod_depth": 1e39 })),
            ],
            1,
        ),
        (vec![add("p", "probe", json!({ "hz": 72.5 }))], 0),
    ];
    for (edits, index) in refusals {
        let before = observe(&running);
        let error = running.apply_edits(&edits).unwrap_err();
        assert_eq!(observe(&running), before, "{error:?}");
        assert_eq!(error.code, RpcErrorCode::InvalidEdit, "{error:?}");
        let edit = error.edit.unwrap();
        assert_eq!(
            (edit.index, edit.reason),
            (index, EditFailureReason::InvalidConfig)
        );
        assert!(edit.message.starts_with("module '"), "{}", edit.message);
        assert!(
            edit.message.contains("expects a finite number, got 1e39")
                || edit
                    .message
                    .contains("probe config 'hz' expects a whole number"),
            "{}",
            edit.message
        );
    }

    running
        .apply_edits(&[
            add("osc3", "oscillator", json!({ "frequency": f32::MAX })),
            add("p", "probe", json!({ "hz": 72.0 })),
        ])
        .unwrap();
    assert_eq!(running.get_control("p", "frequency").unwrap(), number(72.0));
}
