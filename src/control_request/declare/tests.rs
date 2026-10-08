//! Declared tables: resolving keys, coercing values by kind, and reading
//! them back.

use super::*;

const SHAPES: &[&str] = &["sine", "square"];

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "level",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(1.0),
        "Level",
    ),
    ControlDecl::new("shape", DeclKind::Choice(SHAPES), RtValue::U32(0), "Shape"),
    ControlDecl::new(
        "step",
        DeclKind::Integer { min: -2, max: 12 },
        RtValue::I32(0),
        "Step",
    )
    .indexed(3),
    ControlDecl::new("pulse", DeclKind::Event, RtValue::Bool(false), "Pulse"),
    ControlDecl::new("on", DeclKind::Bool, RtValue::Bool(true), "On"),
    ControlDecl::new(
        "cell",
        DeclKind::Number { min: 0.0, max: 8.0 },
        RtValue::F32(0.0),
        "Cell",
    )
    .telemetry(),
];

const TABLE: ControlTable = ControlTable::of(DECLS);

fn coerce(key: &str, value: impl Into<ControlValue>) -> Result<RtValue, String> {
    TABLE.coerce(TABLE.resolve(key).unwrap(), &value.into())
}

#[test]
fn keys_resolve_to_consecutive_indices_and_back() {
    assert_eq!(TABLE.len(), 8);
    let keys = [
        "level", "shape", "step.0", "step.1", "step.2", "pulse", "on", "cell",
    ];
    for (index, key) in keys.iter().enumerate() {
        assert_eq!(
            TABLE.resolve(key),
            Some(ControlIndex(index as u16)),
            "{key}"
        );
        assert_eq!(TABLE.key(ControlIndex(index as u16)).as_deref(), Some(*key));
    }
    for unknown in [
        "step", "step.3", "step.01", "step.-1", "level.0", "levels", "",
    ] {
        assert_eq!(TABLE.resolve(unknown), None, "{unknown}");
    }
    assert_eq!(TABLE.key(ControlIndex(8)), None);
}

#[test]
fn numbers_accept_numeric_text_and_refuse_non_finite() {
    assert_eq!(coerce("level", 0.5), Ok(RtValue::F32(0.5)));
    assert_eq!(
        coerce("level", " 2.5 "),
        Ok(RtValue::F32(2.5)),
        "range is a hint"
    );
    assert!(coerce("level", f32::INFINITY)
        .unwrap_err()
        .contains("finite"));
    assert!(coerce("level", "inf").unwrap_err().contains("finite"));
    assert!(coerce("level", "loud").is_err());
    assert!(coerce("level", true).is_err());
}

#[test]
fn integers_follow_the_config_reader() {
    assert_eq!(coerce("step.1", 3.0), Ok(RtValue::I32(3)));
    assert_eq!(coerce("step.1", "12"), Ok(RtValue::I32(12)));
    assert!(coerce("step.1", 3.5)
        .unwrap_err()
        .contains("whole number from -2 to 12"));
    assert!(coerce("step.1", 13.0).is_err());
    assert!(coerce("step.1", f32::NAN).is_err());
}

#[test]
fn choices_bools_and_events_coerce_by_name() {
    assert_eq!(coerce("shape", "Square"), Ok(RtValue::U32(1)));
    assert!(coerce("shape", "saw")
        .unwrap_err()
        .contains("expects one of"));
    assert!(coerce("shape", 1.0).is_err(), "a choice is written by name");
    assert_eq!(coerce("on", "false"), Ok(RtValue::Bool(false)));
    assert_eq!(coerce("pulse", true), Ok(RtValue::Bool(true)));
    assert!(coerce("pulse", false).is_err());
    assert!(coerce("cell", 1.0).unwrap_err().contains("read-only"));
}

#[test]
fn values_read_back_as_clients_wrote_them() {
    let shape = TABLE.resolve("shape").unwrap();
    assert_eq!(TABLE.value(shape, RtValue::U32(1)), Some("square".into()));
    assert_eq!(TABLE.value(shape, RtValue::U32(2)), None);
    let step = TABLE.resolve("step.2").unwrap();
    assert_eq!(TABLE.value(step, RtValue::I32(-2)), Some((-2.0).into()));

    let metas = TABLE.metas(|index| TABLE.decl(index).unwrap().0.default);
    let keys: Vec<_> = metas.iter().map(|meta| meta.key.as_str()).collect();
    assert_eq!(
        keys,
        ["level", "shape", "step.0", "step.1", "step.2", "pulse", "on", "cell"]
    );
    assert_eq!(metas[1].default, "sine".into());
    assert_eq!(
        metas[2].kind,
        ControlKind::Number {
            min: -2.0,
            max: 12.0
        }
    );
}
