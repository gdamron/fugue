//! Presentation hints: declared once, listed with each number, and left
//! out of the wire form when none is set.

use serde_json::json;

use super::super::{ControlDecl, ControlTable, DeclKind};
use crate::control_request::RtValue;
use crate::{ControlKind, ControlMeta, ControlScale, NumberHints};

fn frequency() -> ControlDecl {
    ControlDecl::new(
        "frequency",
        DeclKind::Number {
            min: 20.0,
            max: 20000.0,
        },
        RtValue::F32(440.0),
        "Frequency",
    )
}

fn listed(decl: ControlDecl) -> ControlKind {
    let table = ControlTable::built(vec![decl]).unwrap();
    table.metas(|_| None).remove(0).kind
}

#[test]
fn a_declared_number_lists_its_hints() {
    let kind = listed(frequency().unit("Hz").log().step(1.0).precision(1));
    let ControlKind::Number { min, max, hints } = kind else {
        panic!("a number");
    };
    assert_eq!((min, max), (20.0, 20000.0), "hints leave the range alone");
    assert_eq!(
        hints,
        NumberHints {
            unit: Some("Hz".into()),
            scale: Some(ControlScale::Log),
            step: Some(1.0),
            precision: Some(1),
        }
    );
}

#[test]
fn a_number_without_hints_lists_none() {
    let ControlKind::Number { hints, .. } = listed(frequency()) else {
        panic!("a number");
    };
    assert!(hints.is_empty());
}

#[test]
fn a_whole_number_may_override_its_derived_step() {
    let decl = ControlDecl::new(
        "bpm",
        DeclKind::Integer { min: 1, max: 300 },
        RtValue::I32(120),
        "Tempo",
    )
    .step(5.0);
    let ControlKind::Number { hints, .. } = listed(decl) else {
        panic!("a number");
    };
    assert_eq!((hints.step, hints.precision), (Some(5.0), Some(0)));
}

#[test]
fn hints_that_cannot_stand_are_refused() {
    let from_zero = ControlDecl::new(
        "attack",
        DeclKind::Number {
            min: 0.0,
            max: 10.0,
        },
        RtValue::F32(0.01),
        "Attack",
    );
    assert!(ControlTable::built(vec![from_zero.clone().log()]).is_err());
    assert!(ControlTable::built(vec![from_zero.clone().step(0.0)]).is_err());
    assert!(ControlTable::built(vec![from_zero.step(f32::NAN)]).is_err());
    let flag = ControlDecl::new("on", DeclKind::Bool, RtValue::Bool(true), "On");
    assert!(ControlTable::built(vec![flag.clone().unit("s")]).is_err());
    assert!(ControlTable::built(vec![flag]).is_ok());
}

#[test]
fn the_wire_form_leaves_absent_hints_out() {
    let plain = ControlMeta::number("level", "Level");
    assert_eq!(
        serde_json::to_value(&plain).unwrap()["kind"],
        json!({"Number": {"min": 0.0, "max": 1.0}}),
        "unchanged for a number without hints"
    );

    let hinted = ControlMeta::number("cutoff", "Cutoff")
        .with_range(20.0, 20000.0)
        .with_unit("Hz")
        .with_log_scale();
    assert_eq!(
        serde_json::to_value(&hinted).unwrap()["kind"],
        json!({"Number": {
            "min": 20.0,
            "max": 20000.0,
            "hints": {"unit": "Hz", "scale": "log"},
        }})
    );
}

#[test]
fn a_listing_without_hints_still_reads() {
    let kind: ControlKind =
        serde_json::from_value(json!({"Number": {"min": 0, "max": 1}})).unwrap();
    assert_eq!(
        kind,
        ControlKind::Number {
            min: 0.0,
            max: 1.0,
            hints: NumberHints::default(),
        }
    );
}

#[test]
fn hint_builders_leave_other_kinds_alone() {
    let flag = ControlMeta::boolean("on", "On", true).with_unit("s");
    assert_eq!(flag.kind, ControlKind::Bool);
    let ranged_after = ControlMeta::number("rate", "Rate")
        .with_unit("Hz")
        .with_range(0.01, 100.0);
    let ControlKind::Number { hints, .. } = ranged_after.kind else {
        panic!("a number");
    };
    assert_eq!(hints.unit.as_deref(), Some("Hz"), "a range keeps its hints");
}
