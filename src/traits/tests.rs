use super::*;

#[test]
fn test_validate_port() {
    let ports = &["audio", "cv", "gate"];

    assert!(validate_port("audio", ports, "input").is_ok());
    assert!(validate_port("cv", ports, "input").is_ok());
    assert!(validate_port("invalid", ports, "input").is_err());
}

#[test]
fn coerce_stringified_values_to_declared_kind() {
    let number = ControlKind::Number {
        min: 0.0,
        max: 1.0,
        hints: NumberHints::default(),
    };
    let boolean = ControlKind::Bool;
    let string = ControlKind::String { options: None };

    // A stringified number/bool lands on a numeric/boolean control: some MCP
    // clients stringify values.
    assert_eq!(
        ControlValue::String("0.74".to_string()).coerced_to(&number),
        ControlValue::Number(0.74)
    );
    assert_eq!(
        ControlValue::String("  3 ".to_string()).coerced_to(&number),
        ControlValue::Number(3.0)
    );
    assert_eq!(
        ControlValue::String("true".to_string()).coerced_to(&boolean),
        ControlValue::Bool(true)
    );
    assert_eq!(
        ControlValue::String("false".to_string()).coerced_to(&boolean),
        ControlValue::Bool(false)
    );

    // Already-correct values pass through untouched.
    assert_eq!(
        ControlValue::Number(0.74).coerced_to(&number),
        ControlValue::Number(0.74)
    );
    assert_eq!(
        ControlValue::Bool(true).coerced_to(&boolean),
        ControlValue::Bool(true)
    );
    assert_eq!(
        ControlValue::String("sine".to_string()).coerced_to(&string),
        ControlValue::String("sine".to_string())
    );

    // A bare number/bool sent to a string control becomes its string form.
    assert_eq!(
        ControlValue::Number(3.0).coerced_to(&string),
        ControlValue::String("3".to_string())
    );
    assert_eq!(
        ControlValue::Bool(true).coerced_to(&string),
        ControlValue::String("true".to_string())
    );

    // Values that cannot represent the kind pass through unchanged, leaving the
    // module setter as the single source of validation errors.
    assert_eq!(
        ControlValue::String("sine".to_string()).coerced_to(&number),
        ControlValue::String("sine".to_string())
    );
    assert_eq!(
        ControlValue::String("maybe".to_string()).coerced_to(&boolean),
        ControlValue::String("maybe".to_string())
    );
}

/// Every built-in number's hints can stand, as the widget reads them: a log
/// scale only over a range above zero, a positive step, at most six
/// decimals. Frequencies are marked log once, here in core.
#[test]
fn built_in_hints_can_stand() {
    let registry = crate::ModuleRegistry::default();
    let mut log_scaled = Vec::new();
    for type_id in registry.types() {
        if registry.is_sink(type_id) {
            continue;
        }
        let Ok(built) = registry.build(type_id, 48_000, &serde_json::json!({})) else {
            continue;
        };
        let Some(surface) = built.control_surface else {
            continue;
        };
        for meta in surface.controls() {
            let ControlKind::Number { min, max, hints } = meta.kind else {
                continue;
            };
            let control = format!("{type_id}.{}", meta.key);
            if hints.scale == Some(ControlScale::Log) {
                assert!(min > 0.0 && min < max, "{control}: log over {min}..{max}");
                log_scaled.push(control.clone());
            }
            if let Some(step) = hints.step {
                assert!(step.is_finite() && step > 0.0, "{control}: step {step}");
            }
            assert!(hints.precision.unwrap_or(0) <= 6, "{control}: precision");
        }
    }
    for control in ["oscillator.frequency", "filter.cutoff", "lfo.rate"] {
        assert!(log_scaled.iter().any(|c| c == control), "{control} is log");
    }
}
