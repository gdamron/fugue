//! The command a browser host's call stands for under the revision rule:
//! each call maps to the daemon command the rule reads, so the host's answer
//! is the daemon's, and parts that do not fit the call are refused.

use super::*;
use crate::{ControlValue, ControlWrite};

fn authored() -> ControlWrite {
    ControlWrite::new("osc", "frequency", ControlValue::Number(330.0))
}

fn performed() -> ControlWrite {
    ControlWrite::performed("osc", "frequency", ControlValue::Number(330.0))
}

fn call(command: &str, writes: Vec<ControlWrite>) -> RpcCommand {
    RpcCommand::revision_call(command, writes, false).unwrap()
}

fn refusal(command: &str, writes: Vec<ControlWrite>, timed: bool) -> RpcError {
    RpcCommand::revision_call(command, writes, timed).unwrap_err()
}

#[test]
fn a_set_control_keeps_its_one_write_whole() {
    let mut write = performed();
    write.priority = Some(3);
    write.source = Some(RequestSource::Agent);
    let RpcCommand::SetControl {
        module_id,
        key,
        value,
        intent,
        source,
        priority,
        timing,
    } = call("set_control", vec![write.clone()])
    else {
        panic!("a set_control stands for a SetControl");
    };
    let kept = ControlWrite {
        module_id,
        key,
        value,
        intent,
        source,
        priority,
        timing,
    };
    assert_eq!(kept, write);
}

#[test]
fn a_set_control_advances_as_its_write_authors() {
    assert!(call("set_control", vec![authored()]).advances_revision_after(true));
    assert!(!call("set_control", vec![performed()]).advances_revision_after(true));
    assert!(!call("set_control", vec![authored()]).advances_revision_after(false));
}

#[test]
fn a_set_controls_advances_once_any_write_authors() {
    let mixed = call("set_controls", vec![performed(), authored()]);
    assert_eq!(
        mixed,
        RpcCommand::SetControls {
            writes: vec![performed(), authored()]
        }
    );
    assert!(mixed.advances_revision_after(true));
    assert!(
        !mixed.advances_revision_after(false),
        "a refused batch leaves it"
    );
    let gesture = call("set_controls", vec![performed(), performed()]);
    assert!(!gesture.advances_revision_after(true));
    assert!(!call("set_controls", Vec::new()).advances_revision_after(true));
}

#[test]
fn an_edit_batch_advances_when_it_commits_unless_it_is_timed() {
    let at_once = call("apply_edits", Vec::new());
    assert!(at_once.advances_revision_after(true));
    assert!(!at_once.advances_revision_after(false));
    let queued = RpcCommand::revision_call("apply_edits", Vec::new(), true).unwrap();
    assert!(queued.edit_timing().is_some_and(EditTiming::is_timed));
    assert!(
        !queued.advances_revision_after(true),
        "it advances as it is adopted"
    );
    assert!(!queued.advances_revision_after(false));
}

#[test]
fn parts_that_do_not_fit_the_call_are_refused() {
    let cases = [
        refusal("set_control", Vec::new(), false),
        refusal("set_control", vec![authored(), authored()], false),
        refusal("set_control", vec![authored()], true),
        refusal("set_controls", vec![authored()], true),
        refusal("apply_edits", vec![authored()], false),
        refusal("load_invention", Vec::new(), false),
    ];
    for error in cases {
        assert_eq!(
            error.code,
            RpcErrorCode::InvalidRequest,
            "{}",
            error.message
        );
    }
}
