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
            format: None,
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

#[test]
fn a_number_its_module_truncates_shows_as_a_whole_number() {
    let count = ControlDecl::new(
        "degree_count",
        DeclKind::Number { min: 1.0, max: 8.0 },
        RtValue::F32(7.0),
        "Scale degrees",
    )
    .whole();
    let ControlKind::Number { hints, .. } = listed(count) else {
        panic!("a number");
    };
    assert_eq!((hints.step, hints.precision), (Some(1.0), Some(0)));
}

#[test]
fn a_note_lists_its_format_on_the_wire() {
    let root = ControlDecl::new(
        "root_note",
        DeclKind::Integer { min: 0, max: 127 },
        RtValue::I32(60),
        "Root MIDI note",
    )
    .note();
    let table = ControlTable::built(vec![root]).unwrap();
    let meta = table.metas(|_| None).remove(0);
    assert_eq!(
        serde_json::to_value(&meta).unwrap()["kind"]["Number"]["hints"],
        json!({"step": 1.0, "precision": 0, "format": "note"})
    );
    let flag = ControlDecl::new("on", DeclKind::Bool, RtValue::Bool(true), "On");
    assert!(
        ControlTable::built(vec![flag.note()]).is_err(),
        "only a number is a note"
    );
}

#[test]
fn a_value_only_its_module_writes_is_listed_read_only() {
    let position = ControlDecl::new(
        "position",
        DeclKind::Number { min: 0.0, max: 1e9 },
        RtValue::F32(0.0),
        "Beats played",
    )
    .telemetry();
    let table = ControlTable::built(vec![position, frequency()]).unwrap();
    let metas = table.metas(|_| None);
    let wire = |meta: &ControlMeta| serde_json::to_value(meta).unwrap();
    assert!(metas[0].read_only);
    assert_eq!(wire(&metas[0])["read_only"], json!(true));
    assert!(!metas[1].read_only);
    assert!(
        wire(&metas[1]).get("read_only").is_none(),
        "a control a person may change keeps its wire form"
    );
    let older: ControlMeta = serde_json::from_value(json!({
        "key": "level", "description": "Level", "default": 0.5,
        "kind": {"Number": {"min": 0, "max": 1}},
    }))
    .unwrap();
    assert!(!older.read_only, "a listing from before reads as writable");
}

#[test]
fn a_hand_built_reading_is_listed_read_only_and_refuses_writes() {
    let step = ControlMeta::number("step", "Current step")
        .with_range(-1.0, 64.0)
        .read_only();
    assert_eq!(
        serde_json::to_value(&step).unwrap()["read_only"],
        json!(true)
    );
    let listing = [step, ControlMeta::number("level", "Level")];
    let refused = crate::traits::check_listed_control(&listing, "step", &3.0.into());
    assert!(refused.unwrap_err().contains("read-only"));
    assert!(crate::traits::check_listed_control(&listing, "level", &0.5.into()).is_ok());
}

#[test]
fn a_one_shot_is_listed_as_an_event() {
    let reset = ControlDecl::new("reset", DeclKind::Bool, RtValue::Bool(false), "Reset").event();
    let flag = ControlDecl::new("on", DeclKind::Bool, RtValue::Bool(true), "On");
    let table = ControlTable::built(vec![reset, flag]).unwrap();
    let metas = table.metas(|_| None);
    let wire = |meta: &ControlMeta| serde_json::to_value(meta).unwrap();
    assert!(metas[0].event);
    assert_eq!(wire(&metas[0])["event"], json!(true));
    assert!(!metas[1].event);
    assert!(
        wire(&metas[1]).get("event").is_none(),
        "a state keeps its wire form"
    );
    let older: ControlMeta = serde_json::from_value(json!({
        "key": "on", "description": "On", "default": true, "kind": "Bool",
    }))
    .unwrap();
    assert!(!older.event, "a listing from before reads as a state");
}
