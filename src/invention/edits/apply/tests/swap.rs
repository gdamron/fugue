//! `swap_module` in an edit batch: the module keeps its id, name, position
//! and every connection its new type has a port for; the rest are removed
//! and reported by edit index; later edits see the new module; a bad swap
//! refuses the batch with nothing changed.

use super::*;
use crate::invention::layout::Position;
use crate::rpc::{EditFailureReason, EditOp, RpcErrorCode, SwappedModule};

/// A sine through a filter whose cutoff an LFO sweeps, beside a second
/// voice; the filter is named and placed.
const SWEPT: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "lfo", "type": "lfo", "config": { "rate": 2.0 } },
        { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 220.0 } },
        { "id": "fx", "type": "filter", "name": "Lead filter", "config": { "cutoff": 800.0 } },
        { "id": "voice", "type": "oscillator", "config": { "waveform": "sine", "frequency": 330.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "fx", "to_port": "audio" },
        { "from": "lfo", "from_port": "bipolar", "to": "fx", "to_port": "cutoff_mod" },
        { "from": "fx", "from_port": "audio", "to": "dac", "to_port": "audio_right" },
        { "from": "voice", "from_port": "audio", "to": "dac", "to_port": "audio_left" }
    ],
    "layout": { "modules": { "fx": { "x": 120, "y": 40 } } }
}"#;

fn swap(id: &str, module_type: &str, config: serde_json::Value) -> StructuralEdit {
    StructuralEdit::SwapModule {
        id: id.into(),
        module_type: module_type.into(),
        config,
    }
}

fn wire(from: &str, from_port: &str, to: &str, to_port: &str) -> RuntimeConnectionInfo {
    RuntimeConnectionInfo {
        from: from.into(),
        from_port: from_port.into(),
        to: to.into(),
        to_port: to_port.into(),
    }
}

fn refused_at(
    running: &mut RunningInvention,
    edits: &[StructuralEdit],
) -> (usize, EditOp, EditFailureReason) {
    let before = observe(running);
    let error = running
        .apply_edits(edits)
        .expect_err("the batch is refused");
    assert_eq!(observe(running), before, "{error:?}");
    assert_eq!(error.code, RpcErrorCode::InvalidEdit, "{error:?}");
    let edit = error.edit.as_deref().expect("an edit detail");
    (edit.index, edit.op, edit.reason)
}

#[test]
fn a_swap_keeps_the_connections_its_new_type_has_ports_for_and_reports_the_rest() {
    let (mut running, pump) = start(SWEPT);
    pump.render(3);
    let generation = running.live.generation();

    let report = running
        .apply_edits(&[swap("fx", "vca", json!({}))])
        .expect("the batch commits");
    assert_eq!(
        report.swapped,
        [SwappedModule {
            edit_index: 0,
            module_id: "fx".into(),
            connections_removed: vec![wire("lfo", "bipolar", "fx", "cutoff_mod")],
        }]
    );
    assert_eq!(report.rebuilt, ["fx"]);
    assert!(report.added.is_empty() && report.removed.is_empty());
    assert_eq!(
        (report.connections_added, report.connections_removed),
        (0, 1)
    );
    assert_eq!(running.live.generation(), generation + 1, "one publication");

    let connections = running.state.lock().unwrap().connections.clone();
    assert!(connections.contains(&wire("osc", "audio", "fx", "audio")));
    assert!(connections.contains(&wire("fx", "audio", "dac", "audio_right")));
    assert!(!connections.contains(&wire("lfo", "bipolar", "fx", "cutoff_mod")));

    let document = running.document().unwrap();
    let fx = document
        .modules
        .iter()
        .find(|spec| spec.id == "fx")
        .unwrap();
    assert_eq!(fx.module_type, "vca");
    assert_eq!(fx.name.as_deref(), Some("Lead filter"), "the name stays");
    assert_eq!(
        document.layout.modules["fx"],
        Position { x: 120.0, y: 40.0 }
    );
    assert_eq!(document.connections.len(), 3);
    // A batch with no swap reports none, and the field stays off the wire.
    let report = (running.apply_edits(&[set("osc", "frequency", number(110.0))])).unwrap();
    assert!(report.swapped.is_empty());
    assert!(serde_json::to_value(&report)
        .unwrap()
        .get("swapped")
        .is_none());
    pump.render(2);
}

#[test]
fn later_edits_see_the_swapped_module_and_its_ports() {
    let (mut running, pump) = start(SWEPT);
    // The new type's port is there to wire at the next index, and its
    // controls to write.
    let report = running
        .apply_edits(&[
            swap("fx", "vca", json!({})),
            connect("lfo", "unipolar", "fx", "level"),
            set("fx", "level", number(0.5)),
            swap("osc", "lfo", json!({ "rate": 4.0 })),
        ])
        .expect("the batch commits");
    assert_eq!(report.rebuilt, ["osc", "fx"]);
    let swapped: Vec<(usize, &str, usize)> = (report.swapped.iter())
        .map(|swap| {
            (
                swap.edit_index,
                swap.module_id.as_str(),
                swap.connections_removed.len(),
            )
        })
        .collect();
    // An LFO has no `audio` output: the oscillator's wire to fx goes.
    assert_eq!(swapped, [(0, "fx", 1), (3, "osc", 1)]);
    assert_eq!(
        report.swapped[1].connections_removed,
        [wire("osc", "audio", "fx", "audio")]
    );
    assert_eq!(
        (report.connections_added, report.connections_removed),
        (1, 2)
    );
    assert_eq!(running.get_control("fx", "level").unwrap(), number(0.5));
    assert_eq!(config_of(&running, "osc"), json!({ "rate": 4.0 }));

    // The old type's port is gone for the next edit.
    let (mut running, _pump) = start(SWEPT);
    let refusal = refused_at(
        &mut running,
        &[
            swap("fx", "vca", json!({})),
            connect("lfo", "bipolar", "fx", "cutoff_mod"),
        ],
    );
    assert_eq!(
        refusal,
        (1, EditOp::Connect, EditFailureReason::UnknownPort)
    );
    pump.render(1);
}

#[test]
fn a_module_the_batch_adds_can_be_swapped_and_one_it_removes_cannot() {
    let (mut running, pump) = start(SWEPT);
    let report = running
        .apply_edits(&[
            add("mod", "oscillator", json!({ "frequency": 2.0 })),
            connect("mod", "audio", "fx", "cutoff_mod"),
            swap("mod", "lfo", json!({ "rate": 3.0 })),
        ])
        .expect("the batch commits");
    assert_eq!(report.added, ["mod"]);
    assert!(report.rebuilt.is_empty());
    assert_eq!(
        report.swapped[0].connections_removed,
        [wire("mod", "audio", "fx", "cutoff_mod")]
    );
    assert_eq!(config_of(&running, "mod"), json!({ "rate": 3.0 }));

    let refusal = refused_at(
        &mut running,
        &[remove("voice"), swap("voice", "noise", json!({}))],
    );
    assert_eq!(
        refusal,
        (1, EditOp::SwapModule, EditFailureReason::UnknownModule)
    );
    pump.render(1);
}

#[test]
fn a_bad_swap_refuses_the_batch_at_its_index_with_nothing_changed() {
    let (mut running, pump) = start(SWEPT);
    let (_twin, twin) = start(SWEPT);
    let retune = set("osc", "frequency", number(440.0));
    let cases = [
        (
            swap("nobody", "vca", json!({})),
            EditFailureReason::UnknownModule,
        ),
        (
            swap("fx", "no_such_type", json!({})),
            EditFailureReason::UnknownModuleType,
        ),
        (
            swap("fx", "oscillator", json!({ "waveform": "bogus" })),
            EditFailureReason::InvalidConfig,
        ),
        (swap("", "vca", json!({})), EditFailureReason::InvalidName),
    ];
    for (bad, reason) in cases {
        let refusal = refused_at(&mut running, &[retune.clone(), bad.clone()]);
        assert_eq!(refusal, (1, EditOp::SwapModule, reason), "{bad:?}");
    }
    // The music never heard any of it.
    assert_eq!(pump.render(8), twin.render(8));
}

#[test]
fn writes_before_a_swap_go_with_the_old_module_and_later_ones_reach_the_new() {
    let (mut running, pump) = start(SWEPT);
    let report = running
        .apply_edits(&[
            set("fx", "cutoff", number(2000.0)),
            swap("fx", "filter", json!({ "cutoff": 300.0 })),
            set("fx", "resonance", number(0.5)),
        ])
        .expect("the batch commits");
    assert_eq!(
        report.rebuilt,
        ["fx"],
        "a fresh instance, as swap_module builds"
    );
    assert_eq!(report.swapped[0].edit_index, 1);
    assert!(report.swapped[0].connections_removed.is_empty());
    assert_eq!(running.get_control("fx", "cutoff").unwrap(), number(300.0));
    assert_eq!(running.get_control("fx", "resonance").unwrap(), number(0.5));
    assert_eq!(config_of(&running, "fx")["cutoff"], json!(300.0));
    pump.render(1);
}
