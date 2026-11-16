//! The `policy` package kind and the `policies` requirement field.

use fugue::pkg::{parse_str, PackageManifest};
use fugue::{ManifestError, PkgValidationError as ValidationError};

const POLICY: &str = include_str!("../src/pkg/examples/policy.json");
const SKILL: &str = include_str!("../src/pkg/examples/skill.json");
const MODULE: &str = include_str!("../src/pkg/examples/module.json");

fn with(json: &str, field: &str, value: serde_json::Value) -> String {
    let mut manifest: serde_json::Value = serde_json::from_str(json).unwrap();
    manifest[field] = value;
    manifest.to_string()
}

fn invalid(json: &str) -> ValidationError {
    match parse_str(json) {
        Err(ManifestError::Invalid(err)) => err,
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn a_policy_entry_must_match_the_policy_kind() {
    parse_str(POLICY).expect("policy example validates");

    let other_entry = with(POLICY, "entry", serde_json::json!({"skill": "SKILL.md"}));
    assert!(matches!(
        invalid(&other_entry),
        ValidationError::KindEntryMismatch {
            kind: "policy",
            entry_kind: "skill"
        }
    ));

    let policy_entry_on_skill = with(SKILL, "entry", serde_json::json!({"policy": "POLICY.md"}));
    assert!(matches!(
        invalid(&policy_entry_on_skill),
        ValidationError::KindEntryMismatch {
            kind: "skill",
            entry_kind: "policy"
        }
    ));
}

#[test]
fn a_policy_package_requires_nothing() {
    for field in ["deps", "policies"] {
        let json = with(
            POLICY,
            field,
            serde_json::json!(["fugue.example.other@^1.0.0"]),
        );
        let err = invalid(&json);
        assert_eq!(err, ValidationError::PolicyPackageRequires { field });
        assert!(err.to_string().contains(&format!("`{field}`")), "{err}");
    }
    let empty = with(POLICY, "policies", serde_json::json!([]));
    parse_str(&empty).expect("an empty policies list requires nothing");
}

#[test]
fn skills_and_agents_accept_policies() {
    let skill = parse_str(SKILL).expect("skill example validates");
    assert_eq!(
        skill.policy_requirements(),
        ["fugue.example.mixing-policy@^0.1.0".to_string()]
    );
    let agent = with(
        include_str!("../src/pkg/examples/agent.json"),
        "policies",
        serde_json::json!(["fugue.example.mixing-policy@^0.1.0"]),
    );
    parse_str(&agent).expect("agent with policies validates");
}

#[test]
fn a_malformed_policy_entry_is_quoted() {
    for bad in [
        "fugue.example.mixing-policy",
        "@^0.1.0",
        "fugue.example.mixing-policy@",
    ] {
        let json = with(SKILL, "policies", serde_json::json!([bad]));
        let err = invalid(&json);
        assert_eq!(err, ValidationError::InvalidPolicyRef(bad.to_string()));
        assert!(err.to_string().contains(&format!("`{bad}`")), "{err}");
    }
}

#[test]
fn other_kinds_refuse_policies() {
    let examples = [
        ("module", MODULE),
        (
            "development",
            include_str!("../src/pkg/examples/development.json"),
        ),
        (
            "invention",
            include_str!("../src/pkg/examples/invention.json"),
        ),
        (
            "sample-pack",
            include_str!("../src/pkg/examples/sample-pack.json"),
        ),
    ];
    for (kind, json) in examples {
        for value in [
            serde_json::json!(["fugue.example.mixing-policy@^0.1.0"]),
            serde_json::json!([]),
        ] {
            let err = invalid(&with(json, "policies", value));
            assert_eq!(err, ValidationError::PoliciesNotAllowed { kind });
        }
    }
}

#[test]
fn a_manifest_without_policies_round_trips_unchanged() {
    let parsed = parse_str(MODULE).unwrap();
    assert_eq!(parsed.policies, None);
    let out = serde_json::to_value(&parsed).unwrap();
    assert!(out.get("policies").is_none(), "{out}");
    let reparsed: PackageManifest = serde_json::from_value(out).unwrap();
    assert_eq!(parsed, reparsed);
}
