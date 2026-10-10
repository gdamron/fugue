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
    ControlDecl::new(
        "note",
        DeclKind::Integer { min: 0, max: 127 },
        RtValue::I32(60),
        "Note",
    )
    .event(),
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
    for index in 0..TABLE.len() {
        let index = ControlIndex(index as u16);
        assert_eq!(TABLE.resolve(&TABLE.key(index).unwrap()), Some(index));
    }
    assert_eq!(TABLE.len(), 8);
    let keys = [
        "level", "shape", "step.0", "step.1", "step.2", "note", "on", "cell",
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
        "step.+0",
        "step.+00",
        "step. 1",
        "step.00",
        "step.100000",
        "step",
        "step.3",
        "step.01",
        "step.-1",
        "level.0",
        "levels",
        "",
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
    assert_eq!(
        coerce("note", 61.0),
        Ok(RtValue::I32(61)),
        "an event carries its value"
    );
    assert!(coerce("cell", 1.0).unwrap_err().contains("read-only"));
}

#[test]
fn values_read_back_as_clients_wrote_them() {
    let shape = TABLE.resolve("shape").unwrap();
    assert_eq!(TABLE.value(shape, RtValue::U32(1)), Some("square".into()));
    assert_eq!(TABLE.value(shape, RtValue::U32(2)), None);
    let step = TABLE.resolve("step.2").unwrap();
    assert_eq!(TABLE.value(step, RtValue::I32(-2)), Some((-2.0).into()));

    let metas = TABLE.metas(|index| (index == ControlIndex(0)).then(|| 0.5.into()));
    let keys: Vec<_> = metas.iter().map(|meta| meta.key.as_str()).collect();
    assert_eq!(
        keys,
        ["level", "shape", "step.0", "step.1", "step.2", "note", "on", "cell"]
    );
    assert_eq!(metas[0].default, 0.5.into(), "what it holds now");
    assert_eq!(metas[1].default, "sine".into(), "else its declared default");
    assert_eq!(
        metas[2].kind,
        ControlKind::Number {
            min: -2.0,
            max: 12.0
        }
    );
}

#[test]
fn an_indexed_control_keeps_its_index_whatever_its_count() {
    const DECLS: &[ControlDecl] = &[
        ControlDecl::new("gain", DeclKind::Bool, RtValue::Bool(true), "Gain").indexed(1),
        ControlDecl::new("unused", DeclKind::Bool, RtValue::Bool(true), "None").indexed(0),
        ControlDecl::new("pan", DeclKind::Bool, RtValue::Bool(true), "Pan"),
    ];
    let table = ControlTable::of(DECLS);
    assert_eq!(table.len(), 2);
    assert_eq!(table.resolve("gain.0"), Some(ControlIndex(0)));
    assert_eq!(table.resolve("gain"), None);
    assert_eq!(table.resolve("unused.0"), None);
    assert_eq!(table.resolve("pan"), Some(ControlIndex(1)));
    assert_eq!(table.key(ControlIndex(0)).as_deref(), Some("gain.0"));
}

#[test]
fn a_table_past_its_index_space_or_exact_integers_is_refused() {
    let flags = |key, count| {
        ControlDecl::new(key, DeclKind::Bool, RtValue::Bool(false), "Flag").indexed(count)
    };
    let full = ControlTable::built(vec![flags("a", u16::MAX), flags("b", 1)]).unwrap();
    assert_eq!(full.len(), MAX_CONTROLS);
    assert_eq!(full.resolve("b.0"), Some(ControlIndex(u16::MAX)));
    assert!(ControlTable::built(vec![flags("a", u16::MAX), flags("b", 2)]).is_err());

    let integer =
        |min, max| ControlDecl::new("n", DeclKind::Integer { min, max }, RtValue::I32(0), "N");
    let table = ControlTable::built(vec![integer(-MAX_EXACT_INTEGER, MAX_EXACT_INTEGER)]).unwrap();
    let n = ControlIndex(0);
    for whole in [MAX_EXACT_INTEGER, -MAX_EXACT_INTEGER, MAX_EXACT_INTEGER - 1] {
        let read = table.value(n, RtValue::I32(whole)).unwrap();
        assert_eq!(table.coerce(n, &read), Ok(RtValue::I32(whole)), "{whole}");
    }
    assert!(ControlTable::built(vec![integer(0, MAX_EXACT_INTEGER + 1)]).is_err());
    assert!(ControlTable::built(vec![integer(i32::MIN, 0)]).is_err());
    let plain_of_two = ControlDecl {
        count: 2,
        ..integer(0, 1)
    };
    assert!(ControlTable::built(vec![plain_of_two]).is_err());
}

#[test]
fn a_payload_reads_back_through_its_surface_not_a_scalar() {
    const DECLS: &[ControlDecl] = &[ControlDecl::new(
        "pattern",
        DeclKind::Payload,
        RtValue::Bool(false),
        "Pattern",
    )];
    let table = ControlTable::of(DECLS);
    let pattern = ControlIndex(0);
    assert_eq!(table.value(pattern, RtValue::Bool(false)), None);
    assert!(table.coerce(pattern, &"[]".into()).is_err());
    let listed = table.metas(|_| Some("[1, 0]".into()));
    assert_eq!(listed[0].default, "[1, 0]".into());
    assert_eq!(table.metas(|_| None)[0].default, "".into());
}

#[test]
fn two_controls_claiming_one_key_are_refused() {
    let plain = |key| ControlDecl::new(key, DeclKind::Bool, RtValue::Bool(false), "Flag");
    let built = |decls: Vec<ControlDecl>| ControlTable::built(decls).map(|_| ());
    let refused = Err("two controls claim the same key");
    assert_eq!(built(vec![plain("gain"), plain("gain")]), refused);
    assert_eq!(
        built(vec![plain("gain").indexed(2), plain("gain").indexed(1)]),
        refused
    );
    assert_eq!(
        built(vec![plain("gain").indexed(2), plain("gain.1")]),
        refused
    );
    assert_eq!(
        built(vec![plain("gain.0"), plain("gain").indexed(1)]),
        refused
    );
    // Spellings the indexed control never claims.
    for other in ["gain.2", "gain.01", "gain.", "gain.x", "gains.0", "gain"] {
        assert_eq!(
            built(vec![plain("gain").indexed(2), plain(other)]),
            Ok(()),
            "{other}"
        );
    }
}

#[test]
fn a_default_its_kind_cannot_hold_is_refused() {
    let with = |kind, default| {
        ControlTable::built(vec![ControlDecl::new("x", kind, default, "X")]).map(|_| ())
    };
    let number = DeclKind::Number { min: 0.0, max: 1.0 };
    let integer = DeclKind::Integer { min: 0, max: 127 };
    let refused = Err("a control's default must be a value its kind holds");
    assert_eq!(with(number, RtValue::F32(f32::NAN)), refused);
    assert_eq!(with(number, RtValue::Bool(true)), refused);
    assert_eq!(with(integer, RtValue::I32(128)), refused);
    assert_eq!(with(DeclKind::Bool, RtValue::F32(1.0)), refused);
    assert_eq!(
        with(DeclKind::Choice(&["a", "b"]), RtValue::U32(2)),
        refused
    );
    assert_eq!(with(DeclKind::Choice(&["a", "b"]), RtValue::U32(1)), Ok(()));
    assert_eq!(
        with(number, RtValue::F32(2.0)),
        Ok(()),
        "the range is a hint"
    );
    let backwards = DeclKind::Integer { min: 1, max: 0 };
    assert!(with(backwards, RtValue::I32(0)).is_err());
}

#[test]
fn a_signed_or_padded_suffix_neither_resolves_nor_collides() {
    let plain = |key| ControlDecl::new(key, DeclKind::Bool, RtValue::Bool(false), "Flag");
    for (first, second) in [
        (plain("gain").indexed(2), plain("gain.+0")),
        (plain("gain.+0"), plain("gain").indexed(2)),
        (plain("gain").indexed(2), plain("gain.+00")),
    ] {
        let table = ControlTable::built(vec![first, second]).unwrap();
        for index in 0..table.len() {
            let index = ControlIndex(index as u16);
            assert_eq!(table.resolve(&table.key(index).unwrap()), Some(index));
        }
    }
}

#[test]
fn a_clamped_integer_takes_any_exact_whole_number_into_its_range() {
    const CLAMPED: &[ControlDecl] = &[ControlDecl::new(
        "count",
        DeclKind::Integer { min: 1, max: 64 },
        RtValue::I32(16),
        "Count",
    )
    .clamped(1.0, 64.0)];
    let table = ControlTable::of(CLAMPED);
    let coerce = |value: f32| table.coerce(ControlIndex(0), &value.into());
    assert_eq!(coerce(8.0), Ok(RtValue::I32(8)));
    assert_eq!(coerce(0.0), Ok(RtValue::I32(1)));
    assert_eq!(coerce(-5.0), Ok(RtValue::I32(1)));
    assert_eq!(coerce(1000.0), Ok(RtValue::I32(64)));
    assert!(coerce(8.5).is_err());
    assert!(coerce(MAX_EXACT_INTEGER as f32 * 2.0).is_err());

    let other_range = [ControlDecl::new(
        "count",
        DeclKind::Integer { min: 1, max: 64 },
        RtValue::I32(16),
        "Count",
    )
    .clamped(0.0, 64.0)];
    assert!(ControlTable::built(other_range.to_vec()).is_err());
    let choice = [
        ControlDecl::new("shape", DeclKind::Choice(SHAPES), RtValue::U32(0), "Shape")
            .clamped(0.0, 1.0),
    ];
    assert!(ControlTable::built(choice.to_vec()).is_err());
}
