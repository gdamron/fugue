use super::*;

/// Numbers an f32 rounds to a whole number they are not.
const MISREAD: &[&str] = &[
    "60.0000001",
    "16777214.5",
    "8388608.5",
    "16777217",
    "-16777217",
    // Whole numbers an f64 would round as well: compared exactly.
    "9007199254740991",
    "9007199254740993",
    "9223372036854775807",
    "-9223372036854775807",
    "18446744073709551615",
];

#[test]
fn a_number_an_f32_misreads_keeps_its_text() {
    for text in MISREAD {
        let number: serde_json::Number = serde_json::from_str(text).unwrap();
        assert_eq!(json_number(&number), ControlValue::String(text.to_string()));
        assert!(
            keeps_text(&ControlValue::String(format!(" {text} "))),
            "{text}"
        );
        let written: ControlValue = deserialize(serde_json::json!(number)).unwrap();
        assert_eq!(written, ControlValue::String(text.to_string()));
    }
}

#[test]
fn every_other_number_narrows_as_before() {
    // Whole numbers an f32 holds, fractions it keeps fractional, and numbers
    // too large for one (left to the finite checks).
    for text in [
        "0",
        "60",
        "-3",
        "16777216",
        "0.74",
        "60.5",
        "1e39",
        "8388607.5",
        "-16777216",
        "1099511627776",
        "-9223372036854775808",
    ] {
        let number: serde_json::Number = serde_json::from_str(text).unwrap();
        let narrowed = number.as_f64().unwrap() as f32;
        assert_eq!(
            json_number(&number),
            ControlValue::Number(narrowed),
            "{text}"
        );
        assert!(
            !keeps_text(&ControlValue::String(text.to_string())),
            "{text}"
        );
    }
    assert!(!keeps_text(&ControlValue::Number(60.0)));
    assert!(!keeps_text(&ControlValue::String("sixty".into())));
    // Spellings an f64 reads that JSON does not.
    assert!(keeps_text(&ControlValue::String("+16777217".into())));
    assert!(!keeps_text(&ControlValue::String(".5".into())));
    // Decimal text reads as an f64 does, past what JSON parsing keeps.
    assert!(keeps_text(&ControlValue::String(
        "10.000000000000001".into()
    )));
}

#[test]
fn deserializing_refuses_what_a_control_value_refuses() {
    for value in [
        serde_json::json!(null),
        serde_json::json!([1]),
        serde_json::json!({}),
    ] {
        assert!(deserialize(value).is_err());
    }
    assert_eq!(
        deserialize(serde_json::json!(true)).unwrap(),
        ControlValue::Bool(true)
    );
}
