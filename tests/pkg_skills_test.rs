//! The `skills` requirement field: which kinds may declare it, and how the
//! resolver locks the skills an agent composes.

use std::collections::BTreeMap;
use std::error::Error;

use fugue::pkg::{locked_edges, parse_str, resolve_package, PackageManifest, PackageProvider};
use fugue::{ManifestError, PkgValidationError as ValidationError};
use semver::Version;

const AGENT: &str = include_str!("../src/pkg/examples/agent.json");

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
fn an_agent_accepts_skills() {
    let json = with(
        AGENT,
        "skills",
        serde_json::json!(["fugue.example.groove@^0.1.0"]),
    );
    let agent = parse_str(&json).expect("an agent with skills validates");
    assert_eq!(
        agent.skill_requirements(),
        ["fugue.example.groove@^0.1.0".to_string()]
    );
}

#[test]
fn a_malformed_skills_entry_is_quoted() {
    for bad in ["fugue.example.groove", "@^0.1.0", "fugue.example.groove@"] {
        let err = invalid(&with(AGENT, "skills", serde_json::json!([bad])));
        assert_eq!(err, ValidationError::InvalidSkillRef(bad.to_string()));
        assert!(err.to_string().contains(&format!("`{bad}`")), "{err}");
    }
}

#[test]
fn other_kinds_refuse_skills() {
    let examples = [
        ("module", include_str!("../src/pkg/examples/module.json")),
        (
            "development",
            include_str!("../src/pkg/examples/development.json"),
        ),
        (
            "invention",
            include_str!("../src/pkg/examples/invention.json"),
        ),
        ("skill", include_str!("../src/pkg/examples/skill.json")),
        (
            "sample-pack",
            include_str!("../src/pkg/examples/sample-pack.json"),
        ),
        ("policy", include_str!("../src/pkg/examples/policy.json")),
    ];
    for (kind, json) in examples {
        for value in [
            serde_json::json!(["fugue.example.groove@^0.1.0"]),
            serde_json::json!([]),
        ] {
            let err = invalid(&with(json, "skills", value));
            assert_eq!(err, ValidationError::SkillsNotAllowed { kind });
            assert!(err.to_string().contains("`skills`"), "{err}");
        }
    }
}

#[test]
fn a_manifest_without_skills_round_trips_unchanged() {
    let parsed = parse_str(AGENT).unwrap();
    assert_eq!(parsed.skills, None);
    let out = serde_json::to_value(&parsed).unwrap();
    assert!(out.get("skills").is_none(), "{out}");
    let reparsed: PackageManifest = serde_json::from_value(out).unwrap();
    assert_eq!(parsed, reparsed);
}

/// In-memory packages, every version of each id.
#[derive(Default)]
struct Packages(BTreeMap<String, Vec<PackageManifest>>);

impl Packages {
    fn add(mut self, manifest: serde_json::Value) -> Self {
        let manifest: PackageManifest = serde_json::from_value(manifest).unwrap();
        self.0
            .entry(manifest.id.clone())
            .or_default()
            .push(manifest);
        self
    }
}

impl PackageProvider for Packages {
    type Acquired = ();

    fn available_versions(&self, id: &str) -> Result<Vec<Version>, Box<dyn Error>> {
        Ok(self
            .0
            .get(id)
            .into_iter()
            .flatten()
            .map(|m| Version::parse(&m.version).unwrap())
            .collect())
    }

    fn acquire(
        &self,
        id: &str,
        version: &Version,
    ) -> Result<(PackageManifest, ()), Box<dyn Error>> {
        let found = self.0[id]
            .iter()
            .find(|m| m.version == version.to_string())
            .cloned()
            .ok_or("no such version")?;
        Ok((found, ()))
    }
}

fn package(id: &str, version: &str, kind: &str, targets: &[&str]) -> serde_json::Value {
    let entry = match kind {
        "skill" => serde_json::json!({"skill": "SKILL.md"}),
        "policy" => serde_json::json!({"policy": "POLICY.md"}),
        "agent" => serde_json::json!({"definition": "agent.json"}),
        _ => serde_json::json!({"development": "voice.json"}),
    };
    serde_json::json!({
        "id": id,
        "version": version,
        "kind": kind,
        "license": "MIT",
        "authors": [{"name": "Test"}],
        "targets": targets,
        "entry": entry,
    })
}

fn skill(id: &str, version: &str) -> serde_json::Value {
    package(id, version, "skill", &["in-graph-agent"])
}

fn agent(skills: &[&str], policies: &[&str]) -> PackageManifest {
    let mut value = package("fugue.test.agent", "1.0.0", "agent", &["in-graph-agent"]);
    value["skills"] = serde_json::json!(skills);
    value["policies"] = serde_json::json!(policies);
    serde_json::from_value(value).unwrap()
}

fn resolve_err(root: &PackageManifest, packages: &Packages) -> String {
    match resolve_package(root, packages) {
        Err(err) => err.to_string(),
        Ok(_) => panic!("expected resolution to fail"),
    }
}

#[test]
fn skills_lock_like_dependencies_once_per_id() {
    let mut shared = skill("fugue.test.groove", "0.1.0");
    shared["policies"] = serde_json::json!(["fugue.test.rules@^1.0"]);
    let packages = Packages::default()
        .add(shared)
        .add(skill("fugue.test.groove", "0.1.2"))
        .add(package(
            "fugue.test.rules",
            "1.0.0",
            "policy",
            &["in-graph-agent"],
        ));
    let root = agent(
        &["fugue.test.groove@^0.1.0", "fugue.test.groove@>=0.1"],
        &[],
    );
    let resolved = resolve_package(&root, &packages).unwrap();
    let ids: Vec<_> = resolved.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["fugue.test.groove"], "the newest skill has no policy");
    assert_eq!(resolved[0].version, "0.1.2");

    let versions = BTreeMap::from([("fugue.test.groove".to_string(), "0.1.2".to_string())]);
    assert_eq!(
        locked_edges(&root, &versions),
        ["fugue.test.groove@0.1.2"],
        "one edge however many requirements name the skill"
    );
}

#[test]
fn a_skill_requirement_on_another_kind_names_the_id_and_kind() {
    for kind in ["development", "policy", "agent"] {
        let packages = Packages::default().add(package(
            "fugue.test.voice",
            "1.0.0",
            kind,
            &["in-graph-agent"],
        ));
        let err = resolve_err(&agent(&["fugue.test.voice@^1.0.0"], &[]), &packages);
        assert!(
            err.contains("fugue.test.voice") && err.contains(&format!("`{kind}`")),
            "{err}"
        );
    }
}

#[test]
fn a_skill_for_external_agents_only_is_refused_naming_the_id() {
    let packages = Packages::default().add(package(
        "fugue.test.groove",
        "1.0.0",
        "skill",
        &["external-agent"],
    ));
    let err = resolve_err(&agent(&["fugue.test.groove@^1.0.0"], &[]), &packages);
    assert!(
        err.contains("fugue.test.groove") && err.contains("in-graph-agent"),
        "{err}"
    );
}

#[test]
fn missing_unsatisfiable_and_conflicting_skills_name_the_id() {
    let err = resolve_err(&agent(&["fugue.test.absent@^1"], &[]), &Packages::default());
    assert!(err.contains("fugue.test.absent"), "{err}");

    let packages = Packages::default()
        .add(skill("fugue.test.groove", "1.0.0"))
        .add(skill("fugue.test.groove", "2.0.0"));
    let err = resolve_err(&agent(&["fugue.test.groove@^3"], &[]), &packages);
    assert!(
        err.contains("fugue.test.groove") && err.contains("^3"),
        "{err}"
    );

    let err = resolve_err(
        &agent(&["fugue.test.groove@^2", "fugue.test.groove@^1"], &[]),
        &packages,
    );
    assert!(
        err.contains("skill conflict") && err.contains("fugue.test.groove"),
        "{err}"
    );
}

#[test]
fn a_skill_already_chosen_as_a_dependency_must_still_fit() {
    let packages = Packages::default().add(package(
        "fugue.test.groove",
        "1.0.0",
        "skill",
        &["external-agent"],
    ));
    let mut root = agent(&["fugue.test.groove@^1"], &[]);
    root.deps = vec!["fugue.test.groove@^1".into()];
    let err = resolve_err(&root, &packages);
    assert!(err.contains("in-graph-agent"), "{err}");
}
