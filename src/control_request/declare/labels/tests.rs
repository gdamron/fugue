//! Labels and preferred editors: declared once, listed with each control
//! only where they add something, and left out of the wire form otherwise.

use super::super::{ControlDecl, ControlTable, DeclKind};
use crate::control_request::RtValue;
use crate::{ControlMeta, ControlWidget};

const SHAPES: &[&str] = &["sine", "sawtooth", "pulse"];

fn width() -> ControlDecl {
    ControlDecl::new(
        "pw",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.5),
        "Pulse width",
    )
}

fn shape() -> ControlDecl {
    ControlDecl::new("shape", DeclKind::Choice(SHAPES), RtValue::U32(0), "Shape")
}

fn listed(decls: Vec<ControlDecl>) -> Vec<ControlMeta> {
    ControlTable::built(decls).unwrap().metas(|_| None)
}

#[test]
fn a_declared_label_and_editor_are_listed_with_the_control() {
    let meta = listed(vec![width()
        .label("Pulse width")
        .widget(ControlWidget::Knob)])
    .remove(0);
    assert_eq!(meta.label.as_deref(), Some("Pulse width"));
    assert_eq!(meta.widget, Some(ControlWidget::Knob));
    let wire = serde_json::to_value(&meta).unwrap();
    assert_eq!(wire["label"], "Pulse width");
    assert_eq!(wire["widget"], "knob");
}

#[test]
fn a_control_declaring_nothing_lists_nothing_new() {
    let meta = listed(vec![width()]).remove(0);
    assert_eq!(
        (meta.label, meta.option_labels.len(), meta.widget),
        (None, 0, None)
    );
    let wire = serde_json::to_value(listed(vec![width()]).remove(0)).unwrap();
    for field in ["label", "option_labels", "widget"] {
        assert!(wire.get(field).is_none(), "{field}: {wire}");
    }
}

#[test]
fn a_label_the_tidied_key_already_gives_is_not_listed() {
    let decl = ControlDecl::new(
        "cutoff_mod_depth",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "Depth",
    );
    let meta = listed(vec![decl.label("Cutoff mod depth")]).remove(0);
    assert_eq!(meta.label, None);
}

#[test]
fn a_choice_lists_only_the_option_labels_that_say_more() {
    let decl = shape()
        .option_labels(&[
            ("sine", "Sine"),
            ("sawtooth", "Saw"),
            ("pulse", "Pulse wave"),
        ])
        .widget(ControlWidget::Segmented);
    let meta = listed(vec![decl]).remove(0);
    let labels: Vec<(&str, &str)> = (meta.option_labels.iter())
        .map(|(option, label)| (option.as_str(), label.as_str()))
        .collect();
    assert_eq!(labels, [("pulse", "Pulse wave"), ("sawtooth", "Saw")]);
    assert_eq!(meta.widget, Some(ControlWidget::Segmented));
}

#[test]
fn an_indexed_label_names_each_index() {
    let decl = ControlDecl::new(
        "lvl",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "Level",
    )
    .indexed(2)
    .label("Level");
    let labels: Vec<_> = listed(vec![decl])
        .into_iter()
        .map(|meta| (meta.key, meta.label))
        .collect();
    assert_eq!(
        labels,
        [
            ("lvl.0".to_string(), Some("Level 0".to_string())),
            ("lvl.1".to_string(), Some("Level 1".to_string()))
        ]
    );
}

#[test]
fn labels_and_editors_that_cannot_stand_are_refused() {
    let refused = [
        width().label(""),
        width().label("  "),
        width().option_labels(&[("sine", "Sine")]),
        shape().option_labels(&[("square", "Square")]),
        shape().option_labels(&[("sine", "")]),
        width().widget(ControlWidget::Menu),
        shape().widget(ControlWidget::Knob),
        ControlDecl::new("on", DeclKind::Bool, RtValue::Bool(false), "On")
            .widget(ControlWidget::Stepper),
    ];
    for decl in refused {
        assert!(ControlTable::built(vec![decl.clone()]).is_err(), "{decl:?}");
    }
    let stepper = ControlDecl::new(
        "steps",
        DeclKind::Integer { min: 1, max: 16 },
        RtValue::I32(4),
        "Steps",
    )
    .widget(ControlWidget::Stepper);
    assert!(ControlTable::built(vec![stepper, shape().widget(ControlWidget::Menu)]).is_ok());
}
