//! A development instance a reload builds sets aside a starting value its
//! control refuses, as a load does, and the reload says so: it applies, and
//! its report names the module, the development and the control. (A value
//! written to an instance the reload keeps is a live write, refused as one.)

use super::*;
use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};

/// A development `seq` exposing a step sequencer's whole-number root note
/// as `note`, its instance `lead` configured by `config`, and a `bass`
/// instance configured by `bass` when given.
fn sequence(config: &str, bass: Option<&str>) -> Invention {
    let bass = bass.map_or(String::new(), |config| {
        format!(r#"{{ "id": "bass", "type": "seq", "config": {config} }},"#)
    });
    doc(&format!(
        r#"{{
        "version": "1.0.0",
        "developments": [{{ "name": "seq", "definition": {{
            "version": "1.0.0",
            "modules": [{{ "id": "s", "type": "step_sequencer" }}],
            "connections": [],
            "outputs": [{{ "name": "gate", "from": "s", "from_port": "gate" }}],
            "controls": [{{ "name": "note", "module": "s", "control": "root_note" }}]
        }} }}],
        "modules": [
            {{ "id": "lead", "type": "seq", "config": {config} }},{bass}
            {{ "id": "dac", "type": "dac" }}
        ],
        "connections": [
            {{ "from": "lead", "from_port": "gate", "to": "dac", "to_port": "audio" }}
        ]
    }}"#
    ))
}

#[test]
fn a_reload_reports_a_starting_value_an_added_development_sets_aside() {
    let _ungoverned = crate::pkg::lock::ungoverned_here();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE)
        .build(sequence(r#"{ "note": 60 }"#, None))
        .unwrap();
    let (mut running, pump) = start_manual(runtime);
    pump.render(2);

    let bass = Some(r#"{ "note": 60.5 }"#);
    let report = running.reload(sequence(r#"{ "note": 60 }"#, bass)).unwrap();
    pump.render(2);
    assert_eq!(report.added, ["bass"], "{report:?}");
    let [warning] = report.warnings.as_slice() else {
        panic!("{report:?}");
    };
    assert!(
        warning.starts_with("module 'bass': development 'seq' control 'note'"),
        "{warning}"
    );
    assert_eq!(
        running.get_control("bass", "note").unwrap(),
        ControlValue::Number(48.0),
        "it starts where its definition starts it"
    );

    let report = running.reload(sequence(r#"{ "note": 62 }"#, bass)).unwrap();
    assert!(report.warnings.is_empty(), "{report:?}");
}
