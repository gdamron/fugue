//! The reader's rules, value by value.

use super::*;
use serde_json::json;

pub(crate) mod closed;
mod guard;
pub(crate) mod probe;
pub(crate) mod registry;

const ROOT_NOTE: ConfigKey = ConfigKey::int::<u8>("root_note");
const SEED: ConfigKey = ConfigKey::seed("seed");
const GAIN: ConfigKey = ConfigKey::float("gain");

fn int_u8(config: Value) -> Result<Option<u8>, String> {
    ConfigReader::new("melody", &config)
        .int::<u8>(&ROOT_NOTE)
        .map_err(|error| error.to_string())
}

fn seed(config: Value) -> Result<Option<u64>, String> {
    ConfigReader::new("melody", &config)
        .int::<u64>(&SEED)
        .map_err(|error| error.to_string())
}

fn gain(config: Value) -> Result<Option<f32>, String> {
    ConfigReader::new("mixer", &config)
        .float(&GAIN)
        .map_err(|error| error.to_string())
}

#[test]
fn an_integer_key_reads_integers_and_whole_floats() {
    assert_eq!(int_u8(json!({ "root_note": 72 })), Ok(Some(72)));
    assert_eq!(int_u8(json!({ "root_note": 72.0 })), Ok(Some(72)));
    assert_eq!(int_u8(json!({ "root_note": 0.0 })), Ok(Some(0)));
    assert_eq!(int_u8(json!({ "root_note": -0.0 })), Ok(Some(0)));
    assert_eq!(int_u8(json!({ "root_note": 255 })), Ok(Some(255)));
}

#[test]
fn an_integer_key_refuses_fractions_and_out_of_range_values() {
    assert_eq!(
        int_u8(json!({ "root_note": 72.5 })),
        Err("melody config 'root_note' expects a whole number from 0 to 255, got 72.5".into())
    );
    for value in [
        json!(256),
        json!(256.0),
        json!(-1),
        json!(-1.0),
        json!(1e300),
    ] {
        let error = int_u8(json!({ "root_note": value.clone() })).unwrap_err();
        assert!(
            error.starts_with("melody config 'root_note' expects a whole number from 0 to 255"),
            "{value}: {error}"
        );
    }
}

#[test]
fn a_declared_range_narrows_the_type_range() {
    const CHANNELS: ConfigKey = ConfigKey::integer("channel_count", 1, 64);
    let read = |value: Value| {
        ConfigReader::new("mixer", &json!({ "channel_count": value }))
            .int::<usize>(&CHANNELS)
            .map_err(|error| error.to_string())
    };
    assert_eq!(read(json!(2.0)), Ok(Some(2)));
    assert_eq!(
        read(json!(0)),
        Err("mixer config 'channel_count' expects a whole number from 1 to 64, got 0".into())
    );
}

#[test]
fn a_seed_reads_whole_floats_up_to_2_pow_53_and_larger_integers() {
    let limit = 9_007_199_254_740_992_u64;
    assert_eq!(seed(json!({ "seed": limit as f64 })), Ok(Some(limit)));
    assert_eq!(seed(json!({ "seed": u64::MAX })), Ok(Some(u64::MAX)));
    assert_eq!(seed(json!({ "seed": limit + 1 })), Ok(Some(limit + 1)));
    let error = seed(json!({ "seed": (limit + 2) as f64 })).unwrap_err();
    assert!(
        error.contains("written as an integer beyond 2^53"),
        "{error}"
    );
    assert!(error.starts_with("melody config 'seed'"), "{error}");
    assert!(seed(json!({ "seed": -1 })).is_err());
    assert!(seed(json!({ "seed": 1e20 })).is_err());
}

#[test]
fn a_float_key_reads_finite_f32_values() {
    assert_eq!(gain(json!({ "gain": 0.5 })), Ok(Some(0.5)));
    assert_eq!(gain(json!({ "gain": 2 })), Ok(Some(2.0)));
    assert_eq!(gain(json!({ "gain": f32::MAX })), Ok(Some(f32::MAX)));
    assert_eq!(gain(json!({ "gain": f32::MIN })), Ok(Some(f32::MIN)));
}

#[test]
fn a_float_key_refuses_numbers_too_large_for_an_f32() {
    assert_eq!(
        gain(json!({ "gain": 1e39 })),
        Err("mixer config 'gain' expects a finite number, got 1e39".into())
    );
    let error = gain(json!({ "gain": -1e39 })).unwrap_err();
    assert!(error.contains("expects a finite number"), "{error}");
}

#[test]
fn nan_and_infinities_read_as_absent_since_json_cannot_hold_them() {
    // serde_json has no NaN or infinity: a Value built from one is null,
    // which reads as absent, as on a document that saved one.
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(Value::from(number), Value::Null);
        assert_eq!(gain(json!({ "gain": number })), Ok(None));
    }
}

#[test]
fn absent_and_null_read_as_absent() {
    assert_eq!(int_u8(json!({})), Ok(None));
    assert_eq!(int_u8(json!({ "root_note": null })), Ok(None));
    assert_eq!(gain(json!(null)), Ok(None));
    assert_eq!(gain(json!({ "gain": null })), Ok(None));
}

#[test]
fn a_value_of_another_type_is_refused() {
    for value in [json!("60"), json!(true), json!({ "n": 1 }), json!([60])] {
        let error = int_u8(json!({ "root_note": value.clone() })).unwrap_err();
        assert!(error.contains("expects a whole number"), "{error}");
        assert!(error.ends_with(&value.to_string()), "{error}");
        let error = gain(json!({ "gain": value.clone() })).unwrap_err();
        assert!(error.contains("expects a finite number"), "{error}");
    }
}

#[test]
fn a_long_value_is_cut_short_in_the_message() {
    let error = gain(json!({ "gain": "x".repeat(500) })).unwrap_err();
    assert!(error.len() < 120, "{error}");
    assert!(error.ends_with('…'), "{error}");
}

#[test]
fn value_helpers_read_elements_and_refuse_through_the_reader() {
    let config = json!({ "levels": [0.5, 1e39], "degrees": [0, 2.0, 4.5] });
    let reader = ConfigReader::new("mixer", &config);
    let levels = reader.get("levels").and_then(Value::as_array).unwrap();
    assert_eq!(finite_f32(&levels[0]), Ok(0.5));
    let refusal = finite_f32(&levels[1]).unwrap_err();
    assert_eq!(
        reader.refuse("levels[1]", refusal).to_string(),
        "mixer config 'levels[1]' expects a finite number, got 1e39"
    );
    let degrees = reader.get("degrees").and_then(Value::as_array).unwrap();
    assert_eq!(whole_number::<i32>(&degrees[1]), Ok(2));
    assert!(whole_number::<i32>(&degrees[2]).is_err());
    assert_eq!(whole_number_in::<i8>(&json!(-3.0), -12, 12), Ok(-3));
    assert!(whole_number_in::<i8>(&json!(13), -12, 12).is_err());
}

#[test]
fn an_array_key_reads_finite_numbers_and_refuses_the_rest_by_path() {
    const LEVELS: ConfigKey = ConfigKey::json("levels");
    let floats = |config: Value| {
        ConfigReader::new("mixer", &config)
            .floats(&LEVELS)
            .map_err(|error| error.to_string())
    };
    assert_eq!(floats(json!({})), Ok(None));
    assert_eq!(floats(json!({ "levels": null })), Ok(None));
    assert_eq!(floats(json!({ "levels": [] })), Ok(Some(vec![])));
    assert_eq!(
        floats(json!({ "levels": [0.5, 1, -2.25] })),
        Ok(Some(vec![0.5, 1.0, -2.25]))
    );
    assert_eq!(
        floats(json!({ "levels": [0.5, "x"] })),
        Err("mixer config 'levels[1]' expects a finite number, got \"x\"".into())
    );
    assert_eq!(
        floats(json!({ "levels": { "0": 1 } })),
        Err("mixer config 'levels' expects an array of numbers, got {\"0\":1}".into())
    );
    // The whole-value refusal prints the value as written, as it always has.
    assert_eq!(
        floats(json!({ "levels": 1e39 })),
        Err("mixer config 'levels' expects an array of numbers, got 1e+39".into())
    );
}

#[test]
fn an_array_key_reads_whole_numbers_and_refuses_the_rest_by_path() {
    const DEGREES: ConfigKey = ConfigKey::json("degrees");
    let ints = |config: Value| {
        ConfigReader::new("melody", &config)
            .whole_numbers::<i32>(&DEGREES)
            .map_err(|error| error.to_string())
    };
    assert_eq!(ints(json!({})), Ok(None));
    assert_eq!(ints(json!({ "degrees": null })), Ok(None));
    assert_eq!(
        ints(json!({ "degrees": [0, 4.0, -7] })),
        Ok(Some(vec![0, 4, -7]))
    );
    assert_eq!(
        ints(json!({ "degrees": [0, 4.5] })),
        Err("melody config 'degrees[1]' expects a whole number from -2147483648 to 2147483647, got 4.5".into())
    );
    assert_eq!(
        ints(json!({ "degrees": 3 })),
        Err("melody config 'degrees' expects an array of numbers, got 3".into())
    );
}
