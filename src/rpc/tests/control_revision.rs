//! When a control write advances the revision: once per call that authors,
//! never for a performed one, and not for one refused with nothing written.

use super::*;
use crate::{ControlValue, ControlWrite};

fn one(intent: ControlWriteIntent) -> RpcCommand {
    RpcCommand::SetControl {
        module_id: "osc".to_string(),
        key: "frequency".to_string(),
        value: ControlValue::Number(440.0),
        intent,
        source: None,
        priority: None,
        timing: Default::default(),
    }
}

fn batch(writes: Vec<ControlWrite>) -> RpcCommand {
    RpcCommand::SetControls { writes }
}

#[test]
fn an_authored_write_advances_once_it_is_made() {
    let authored = one(ControlWriteIntent::Author);
    assert!(authored.advances_revision_after(true));
    let mixed = batch(vec![
        ControlWrite::performed("osc", "frequency", ControlValue::Number(1.0)),
        ControlWrite::new("mix", "master", ControlValue::Number(0.5)),
        ControlWrite::new("mix", "level.0", ControlValue::Number(0.2)),
    ]);
    assert!(mixed.advances_revision_after(true));
}

#[test]
fn a_performed_write_never_advances() {
    let performed = one(ControlWriteIntent::Perform);
    assert!(!performed.advances_revision_after(true));
    assert!(!performed.advances_revision_after(false));
    let gesture = batch(vec![
        ControlWrite::performed("osc", "frequency", ControlValue::Number(1.0)),
        ControlWrite::performed("mix", "master", ControlValue::Number(0.5)),
    ]);
    assert!(!gesture.advances_revision_after(true));
}

#[test]
fn a_refused_control_write_leaves_the_revision() {
    // The control rule refuses a batch with nothing written, so the
    // revision stays, as for a refused ApplyEdits.
    assert!(!one(ControlWriteIntent::Author).advances_revision_after(false));
    let authored = batch(vec![ControlWrite::new(
        "mix",
        "master",
        ControlValue::Number(0.5),
    )]);
    assert!(!authored.advances_revision_after(false));
}

#[test]
fn a_control_write_still_needs_no_ticket() {
    let authored = batch(vec![ControlWrite::new(
        "mix",
        "master",
        ControlValue::Number(0.5),
    )]);
    assert!(!authored.is_all_or_nothing());
    assert!(!authored.requires_ticket());
    assert!(!one(ControlWriteIntent::Author).requires_ticket());
}
