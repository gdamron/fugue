use std::path::PathBuf;

use fugue::pkg::sample_pack::{parse_path, parse_str, validate};
use fugue::{SamplePackError, SamplePackManifest, SamplePackValidationError as ValidationError};

fn example() -> &'static str {
    include_str!("../src/pkg/examples/sample-pack.samples.json")
}

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/pkg/examples")
}

#[test]
fn example_parses_validates_and_roundtrips() {
    let parsed = parse_path(examples_dir().join("sample-pack.samples.json")).expect("parse");

    let json = serde_json::to_string_pretty(&parsed).expect("reserialize");
    let reparsed: SamplePackManifest = serde_json::from_str(&json).expect("re-parse");
    validate(&reparsed).expect("re-parse validates");
    assert_eq!(parsed, reparsed, "round-trip mismatch");
}

#[test]
fn per_file_overrides_fall_back_to_pack_values() {
    let pack = parse_str(example()).expect("parse");

    let kick = &pack.files[0];
    assert_eq!(pack.license_for(kick), "CC0-1.0");
    assert_eq!(pack.attribution_for(kick)[0].name, "Example Sampler");
    assert_eq!(pack.sample_rate_for(kick), Some(48000));

    let brk = &pack.files[2];
    assert_eq!(pack.license_for(brk), "CC-BY-4.0");
    assert_eq!(pack.attribution_for(brk)[0].name, "Break Creator");
    assert_eq!(pack.sample_rate_for(brk), Some(44100));
}

#[test]
fn single_declared_rate_is_inherited_by_files() {
    let json = r#"{
        "license": "CC0-1.0",
        "sample_rate": [48000],
        "files": [{ "path": "samples/pulse.wav" }]
    }"#;
    let pack = parse_str(json).expect("parse");
    assert_eq!(pack.sample_rate_for(&pack.files[0]), Some(48000));
}

#[test]
fn rejects_empty_license() {
    let json = example().replace("\"license\": \"CC0-1.0\"", "\"license\": \"\"");
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        SamplePackError::Invalid(ValidationError::EmptyLicense)
    ));
}

#[test]
fn rejects_empty_sample_rates() {
    let json = example().replace("\"sample_rate\": [44100, 48000]", "\"sample_rate\": []");
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        SamplePackError::Invalid(ValidationError::EmptySampleRates)
    ));
}

#[test]
fn rejects_no_files() {
    let json = r#"{
        "license": "CC0-1.0",
        "sample_rate": [48000],
        "files": []
    }"#;
    let err = parse_str(json).unwrap_err();
    assert!(matches!(
        err,
        SamplePackError::Invalid(ValidationError::NoFiles)
    ));
}

#[test]
fn rejects_paths_that_escape_the_package() {
    for path in ["../outside.wav", "/etc/passwd", "samples/./kick.wav", ""] {
        let json = format!(
            r#"{{
                "license": "CC0-1.0",
                "sample_rate": [48000],
                "files": [{{ "path": "{path}" }}]
            }}"#
        );
        let err = parse_str(&json).unwrap_err();
        assert!(
            matches!(
                err,
                SamplePackError::Invalid(ValidationError::InvalidFilePath(_))
            ),
            "`{path}` should be rejected"
        );
    }
}

#[test]
fn rejects_duplicate_file_paths() {
    let json = r#"{
        "license": "CC0-1.0",
        "sample_rate": [48000],
        "files": [
            { "path": "samples/kick.wav" },
            { "path": "samples/kick.wav" }
        ]
    }"#;
    let err = parse_str(json).unwrap_err();
    assert!(matches!(
        err,
        SamplePackError::Invalid(ValidationError::DuplicateFilePath(_))
    ));
}

#[test]
fn rejects_undeclared_per_file_rate() {
    let json = r#"{
        "license": "CC0-1.0",
        "sample_rate": [48000],
        "files": [{ "path": "samples/kick.wav", "sample_rate": 22050 }]
    }"#;
    let err = parse_str(json).unwrap_err();
    assert!(matches!(
        err,
        SamplePackError::Invalid(ValidationError::UndeclaredSampleRate { rate: 22050, .. })
    ));
}

#[test]
fn rejects_ambiguous_file_rate_when_pack_declares_several() {
    let json = r#"{
        "license": "CC0-1.0",
        "sample_rate": [44100, 48000],
        "files": [
            { "path": "samples/kick.wav" },
            { "path": "samples/snare.wav", "sample_rate": 44100 },
            { "path": "samples/hat.wav", "sample_rate": 48000 }
        ]
    }"#;
    let err = parse_str(json).unwrap_err();
    assert!(matches!(
        err,
        SamplePackError::Invalid(ValidationError::AmbiguousSampleRate { .. })
    ));
}

#[test]
fn rejects_declared_rate_no_file_uses() {
    let json = r#"{
        "license": "CC0-1.0",
        "sample_rate": [44100, 48000],
        "files": [{ "path": "samples/kick.wav", "sample_rate": 48000 }]
    }"#;
    let err = parse_str(json).unwrap_err();
    assert!(matches!(
        err,
        SamplePackError::Invalid(ValidationError::UnusedSampleRate(44100))
    ));
}

#[test]
fn rejects_inverted_slice_range() {
    let json = example().replace(
        "{ \"start_frames\": 22050, \"end_frames\": 44100, \"name\": \"snare\" }",
        "{ \"start_frames\": 44100, \"end_frames\": 44100, \"name\": \"snare\" }",
    );
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        SamplePackError::Invalid(ValidationError::InvalidSliceRange { index: 1, .. })
    ));
}

#[test]
fn rejects_duplicate_slice_names() {
    let json = example().replace("\"name\": \"snare\"", "\"name\": \"kick\"");
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        SamplePackError::Invalid(ValidationError::DuplicateSliceName { .. })
    ));
}

#[test]
fn rejects_empty_slice_name() {
    let json = example().replace("\"name\": \"snare\"", "\"name\": \" \"");
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        SamplePackError::Invalid(ValidationError::EmptySliceName { index: 1, .. })
    ));
}

#[test]
fn rejects_empty_attribution_name() {
    let json = example().replace("\"name\": \"Break Creator\"", "\"name\": \"\"");
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        SamplePackError::Invalid(ValidationError::EmptyAttributionName)
    ));
}

#[cfg(feature = "rpc-schema")]
#[test]
fn schema_serializes() {
    let schema = schemars::schema_for!(SamplePackManifest);
    let json = serde_json::to_string(&schema).expect("schema serializes");
    assert!(json.contains("SamplePackManifest"));
}
