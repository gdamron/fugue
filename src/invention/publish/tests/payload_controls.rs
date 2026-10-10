//! Declared payload controls on a live graph: a write is built on the
//! control thread, applied from the next block without allocating, read
//! back as written, and the value it replaces is freed by the reclaimer,
//! never on the audio thread (debug builds panic if it is).

use super::migrated::{block_with_locks_held, read, write};
use super::requests::counted_block;
use super::*;
use crate::invention::declared::Route;
use crate::test_support::tape::{freed, TapeFactory};
use crate::{ControlValue, ModuleFactory};

const DAC: &str = r#"{
    "version": "1.0.0",
    "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
    "connections": []
}"#;

/// A tape holding `[0.25]`, into the dac.
fn rig() -> Rig {
    let mut rig = Rig::new(DAC);
    rig.registry.register(TapeFactory);
    rig.adopt_registry();
    let tape = rig.build("tape", "tape", serde_json::json!({ "notes": [0.25] }));
    rig.live
        .edit(|change| {
            change.upsert("tape", tape);
            change.connect(RoutingConnection {
                from_module: "tape".into(),
                from_port: "out".into(),
                to_module: "dac".into(),
                to_port: "audio".into(),
            })?;
            Ok(())
        })
        .unwrap();
    rig.render(1);
    rig
}

fn output(rig: &Rig) -> f32 {
    rig.graph.modules["tape"].module().output_block(0)[0]
}

#[test]
fn a_payload_applies_from_the_next_block_without_allocating() {
    let mut rig = rig();
    assert_eq!(output(&rig), 0.25);
    assert_eq!(read(&rig, "tape", "notes"), "[0.25]".into());

    write(&rig, "tape", "notes", " [0.5, 0.25] ".into());
    assert_eq!(
        read(&rig, "tape", "notes"),
        "[0.5,0.25]".into(),
        "as written"
    );
    assert_eq!(output(&rig), 0.25, "not before the next block");
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(output(&rig), 0.75);

    write(&rig, "tape", "gain", 0.5.into());
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(output(&rig), 0.375, "scalars still apply beside it");
    let listed = rig.live.control_surfaces.lock().unwrap()["tape"].controls();
    assert_eq!(listed[1].default, "[0.5,0.25]".into());
}

#[test]
fn replaced_payloads_are_freed_by_the_reclaimer() {
    let mut rig = rig();
    rig.live.reclaim();
    let before = freed();
    write(&rig, "tape", "notes", "[1]".into());
    write(&rig, "tape", "notes", "[2]".into());
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(output(&rig), 2.0, "the last write wins");
    assert_eq!(freed(), before, "retired, not freed, on the audio thread");
    rig.live.reclaim();
    assert_eq!(
        freed(),
        before + 2,
        "the built value and the superseded one"
    );
}

#[test]
fn a_payload_that_does_not_parse_is_refused_and_changes_nothing() {
    let mut rig = rig();
    let surface = rig.live.control_surfaces.lock().unwrap()["tape"].clone();
    let surfaces = rig.live.control_surfaces.lock().unwrap().clone();
    for bad in [ControlValue::from("[1, "), ControlValue::Number(1.0)] {
        assert!(surface.validate_control("notes", &bad, &surfaces).is_err());
        assert!(surface.set_control("notes", bad).is_err());
    }
    assert!(surface
        .validate_control("notes", &"[3]".into(), &surfaces)
        .is_ok());
    rig.render(1);
    assert_eq!(read(&rig, "tape", "notes"), "[0.25]".into());
    assert_eq!(output(&rig), 0.25);
}

#[test]
fn a_payload_block_renders_while_the_control_locks_are_held() {
    let mut rig = rig();
    write(&rig, "tape", "notes", "[0.5]".into());
    assert_eq!(block_with_locks_held(&mut rig, &[]), Ok((0, 0)));
    assert_eq!(output(&rig), 0.5);
}

#[test]
fn a_payload_written_while_building_reaches_its_module_when_bound() {
    let mut built = TapeFactory.build(48_000, &serde_json::json!({})).unwrap();
    let surface = built.control_surface.take().unwrap();
    surface.set_control("notes", "[1, 2]".into()).unwrap();
    assert_eq!(surface.get_control("notes").unwrap(), "[1.0,2.0]".into());
    let module = built.module.module_mut();
    surface.bind(Route::Inner, module);
    module.process(1);
    assert_eq!(module.output_block(0)[0], 3.0);
}

/// A recorded write leaves the JSON text a client wrote in the document's
/// config; the module builds from it as from the array.
#[test]
fn a_payload_rebuilds_from_the_text_a_write_records() {
    let text = TapeFactory.build(48_000, &serde_json::json!({ "notes": " [1, 2] " }));
    let surface = text.unwrap().control_surface.unwrap();
    assert_eq!(surface.get_control("notes").unwrap(), "[1.0,2.0]".into());
}
