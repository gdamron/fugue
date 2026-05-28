use std::path::PathBuf;

use fugue::pkg::{parse_path, parse_str, validate, Capability, DepRef, PackageManifest};
use fugue::{ManifestError, PackageKind, PkgValidationError as ValidationError};

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/pkg/examples")
}

const KINDS: &[(&str, PackageKind)] = &[
    ("module.json", PackageKind::Module),
    ("development.json", PackageKind::Development),
    ("invention.json", PackageKind::Invention),
    ("skill.json", PackageKind::Skill),
    ("agent-definition.json", PackageKind::AgentDefinition),
    ("sample-pack.json", PackageKind::SamplePack),
];

#[test]
fn each_example_parses_validates_and_roundtrips() {
    let dir = examples_dir();
    for (file, expected_kind) in KINDS {
        let path = dir.join(file);
        let parsed = parse_path(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(parsed.kind, *expected_kind, "{file}: kind mismatch");

        let json = serde_json::to_string_pretty(&parsed).expect("reserialize");
        let reparsed: PackageManifest = serde_json::from_str(&json).expect("re-parse");
        validate(&reparsed).expect("re-parse validates");
        assert_eq!(parsed, reparsed, "{file}: round-trip mismatch");
    }
}

#[test]
fn rejects_invalid_id() {
    let json = include_str!("../src/pkg/examples/module.json").replace(
        "\"id\": \"fugue.example.reverb\"",
        "\"id\": \"NotReverseDns\"",
    );
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        ManifestError::Invalid(ValidationError::InvalidId(_))
    ));
}

#[test]
fn rejects_invalid_version() {
    let json = include_str!("../src/pkg/examples/module.json")
        .replace("\"version\": \"1.0.0\"", "\"version\": \"1.0\"");
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        ManifestError::Invalid(ValidationError::InvalidVersion(_))
    ));
}

#[test]
fn rejects_kind_entry_mismatch() {
    let json = include_str!("../src/pkg/examples/module.json")
        .replace("\"kind\": \"module\"", "\"kind\": \"invention\"");
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        ManifestError::Invalid(ValidationError::KindEntryMismatch { .. })
    ));
}

#[test]
fn rejects_empty_targets() {
    let json = include_str!("../src/pkg/examples/module.json").replace(
        "\"targets\": [\"claude-code\", \"in-graph-agent\"]",
        "\"targets\": []",
    );
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        ManifestError::Invalid(ValidationError::EmptyTargets)
    ));
}

#[test]
fn rejects_unknown_capability() {
    let json = include_str!("../src/pkg/examples/module.json")
        .replace("\"capabilities\": [\"random\"]", "\"capabilities\": [\"wat:nope\"]");
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        ManifestError::Invalid(ValidationError::UnknownCapability(_))
    ));
}

#[test]
fn rejects_malformed_dep_ref() {
    let json = include_str!("../src/pkg/examples/development.json").replace(
        "\"deps\": [\"fugue.core.sequencer@^1.0.0\"]",
        "\"deps\": [\"no-at-sign\"]",
    );
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        ManifestError::Invalid(ValidationError::InvalidDepRef(_))
    ));
}

#[test]
fn rejects_empty_authors() {
    let json = include_str!("../src/pkg/examples/module.json").replace(
        "\"authors\": [{ \"name\": \"Example Audio\", \"url\": \"https://example.com\" }]",
        "\"authors\": []",
    );
    let err = parse_str(&json).unwrap_err();
    assert!(matches!(
        err,
        ManifestError::Invalid(ValidationError::EmptyAuthors)
    ));
}

#[test]
fn capability_parser_recognizes_known_prefixes() {
    assert_eq!(Capability::parse("random"), Some(Capability::Random));
    assert_eq!(Capability::parse("time"), Some(Capability::Time));
    assert_eq!(
        Capability::parse("fs:read:samples/"),
        Some(Capability::FsRead("samples/".to_string()))
    );
    assert_eq!(
        Capability::parse("fs:write:logs/"),
        Some(Capability::FsWrite("logs/".to_string()))
    );
    assert_eq!(
        Capability::parse("net:example.com"),
        Some(Capability::Net("example.com".to_string()))
    );
    assert!(Capability::parse("fs:exec:/").is_none());
    assert!(Capability::parse("fs:read:").is_none());
    assert!(Capability::parse("net:").is_none());
    assert!(Capability::parse("totally-unknown").is_none());
}

#[test]
fn dep_ref_parses_id_and_requirement() {
    let dep = DepRef::parse("fugue.core.reverb@^1.0.0").unwrap();
    assert_eq!(dep.id, "fugue.core.reverb");
    assert_eq!(dep.requirement, "^1.0.0");
    assert!(DepRef::parse("missing-at-sign").is_none());
    assert!(DepRef::parse("@no-id").is_none());
    assert!(DepRef::parse("no-req@").is_none());
}

#[cfg(feature = "rpc-schema")]
#[test]
fn schema_serializes() {
    let schema = schemars::schema_for!(PackageManifest);
    let json = serde_json::to_string(&schema).expect("schema serializes");
    assert!(json.contains("PackageManifest"));
}
