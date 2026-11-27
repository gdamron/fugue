use super::*;
use crate::rpc::MAX_INSPECTION_VALUE_BYTES;
use serde_json::json;

pub(super) const SAVED: &str = r#"{"title":"Sketch","modules":[
    {"id":"voice","type":"oscillator","config":{"frequency":220,"waveform":"sine"}},
    {"id":"hat","type":"noise"},
    {"id":"out","type":"dac"}],
  "connections":[{"from":"voice","from_port":"out","to":"out","to_port":"left"},
    {"from":"hat","from_port":"out","to":"out","to_port":"right"}],
  "assets":{"kick":{"path":"kick.wav"}}}"#;

pub(super) const CURRENT: &str = r#"{"title":"Sketch in A","modules":[
    {"id":"voice","type":"oscillator","name":"Lead","config":{"frequency":440.0,"waveform":"sine","detune":3}},
    {"id":"out","type":"dac"},
    {"id":"bass","type":"oscillator","config":{"frequency":55}}],
  "connections":[{"from":"voice","from_port":"out","to":"out","to_port":"left"},
    {"from":"bass","from_port":"out","to":"out","to_port":"right"}],
  "assets":{"kick":{"path":"kick-2.wav"}}}"#;

fn compare(saved: &str, current: &str, sounding: &[SoundingValue]) -> Differences {
    let (saved, current) = (
        Invention::from_json(saved).unwrap(),
        Invention::from_json(current).unwrap(),
    );
    differences(&saved, &current, sounding).unwrap()
}

pub(super) fn sounding(module_id: &str, key: &str, value: Value) -> SoundingValue {
    SoundingValue {
        module_id: module_id.into(),
        key: key.into(),
        value,
    }
}

#[test]
fn lists_each_kind_of_change_modules_in_document_order() {
    let found = compare(SAVED, CURRENT, &[]);
    assert_eq!(
        serde_json::to_value(&found.changes).unwrap(),
        json!([
            {"change":"module_removed","id":"hat","type":"noise"},
            {"change":"module_changed","id":"voice","field":"name","current":"Lead"},
            {"change":"config_changed","module_id":"voice","key":"frequency","saved":220,"current":440.0},
            {"change":"config_changed","module_id":"voice","key":"detune","current":3},
            {"change":"module_added","id":"bass","type":"oscillator"},
            {"change":"connection_removed","connection":{"from":"hat","from_port":"out","to":"out","to_port":"right"}},
            {"change":"connection_added","connection":{"from":"bass","from_port":"out","to":"out","to_port":"right"}},
            {"change":"piece_changed","field":"assets","name":"kick","saved":{"path":"kick.wav"},"current":{"path":"kick-2.wav"}},
            {"change":"piece_changed","field":"title","saved":"Sketch","current":"Sketch in A"},
        ])
    );
    assert!(found.performed.is_empty());
}

#[test]
fn the_same_piece_has_nothing_to_list() {
    assert_eq!(compare(SAVED, SAVED, &[]), Differences::default());
    // A number written another way and an empty config are the same.
    let respelled = SAVED
        .replace("220", "220.0")
        .replace(r#""type":"noise""#, r#""type":"noise","config":{}"#);
    assert!(compare(SAVED, &respelled, &[]).changes.is_empty());
}

#[test]
fn developments_compare_by_name() {
    let with = |gain: f64| {
        json!({"modules":[], "connections":[], "developments":[
            {"name":"pad","definition":{"modules":[{"id":"g","type":"vca","config":{"gain":gain}}],"connections":[]}},
            {"name":"lead","definition":{"modules":[],"connections":[]}}]})
        .to_string()
    };
    let found = compare(&with(0.5), &with(0.25), &[]);
    assert_eq!(found.changes.len(), 1);
    let DiffEntry::PieceChanged { field, name, .. } = &found.changes[0] else {
        panic!("{:?}", found.changes)
    };
    assert_eq!(
        (field.as_str(), name.as_deref()),
        ("developments", Some("pad"))
    );
}

#[test]
fn performed_values_stand_apart_from_the_document() {
    let values = [
        // The document holds 220 as an f32 reads it: not performed.
        sounding("voice", "frequency", json!(220.0)),
        sounding("voice", "waveform", json!("saw")),
        // Nothing in the document for it: it plays its default there.
        sounding("hat", "level", json!(0.5)),
        sounding("voice", "waveform", json!("square")),
    ];
    let found = compare(SAVED, SAVED, &values);
    assert!(found.changes.is_empty());
    assert_eq!(
        serde_json::to_value(&found.performed).unwrap(),
        json!([
            {"module_id":"hat","key":"level","sounding":0.5},
            {"module_id":"voice","key":"waveform","document":"sine","sounding":"square"},
        ])
    );
}

#[test]
fn a_large_value_is_left_out() {
    let long = "x".repeat(MAX_INSPECTION_VALUE_BYTES);
    let current = SAVED.replace(r#""title":"Sketch""#, &format!(r#""title":"{long}""#));
    let found = compare(SAVED, &current, &[]);
    let DiffEntry::PieceChanged { values, .. } = &found.changes[0] else {
        panic!("{:?}", found.changes)
    };
    assert!(values.omitted && values.saved.is_none() && values.current.is_none());
}
