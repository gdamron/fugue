use super::*;
use std::cell::RefCell;
use std::collections::HashMap;

fn ver(v: &str) -> Version {
    Version::parse(v).unwrap()
}

#[test]
fn select_version_picks_highest_satisfying() {
    let available = vec![ver("1.0.0"), ver("1.4.0"), ver("2.0.0")];
    assert_eq!(
        select_version(&available, &VersionReq::parse("^1.0").unwrap()),
        Some(ver("1.4.0"))
    );
    assert_eq!(
        select_version(&available, &VersionReq::parse(">=3.0").unwrap()),
        None
    );
}

#[test]
fn dependency_edges_maps_declared_deps_to_resolved_versions() {
    let versions = BTreeMap::from([
        ("a".to_string(), "1.2.0".to_string()),
        ("b".to_string(), "0.3.0".to_string()),
    ]);
    let edges = dependency_edges(&["a@^1.0".into(), "b@~0.3".into()], &versions);
    assert_eq!(edges, vec!["a@1.2.0".to_string(), "b@0.3.0".to_string()]);
}

/// A provider with in-memory fixtures: id -> manifests by version.
struct FakeProvider {
    packages: HashMap<String, Vec<PackageManifest>>,
    acquired: RefCell<Vec<String>>,
}

impl FakeProvider {
    fn new() -> Self {
        Self {
            packages: HashMap::new(),
            acquired: RefCell::new(Vec::new()),
        }
    }

    fn add(self, id: &str, version: &str, deps: &[&str]) -> Self {
        self.add_manifest(manifest(id, version, "development", deps, None))
    }

    fn add_manifest(mut self, manifest: PackageManifest) -> Self {
        self.packages
            .entry(manifest.id.clone())
            .or_default()
            .push(manifest);
        self
    }
}

fn manifest(
    id: &str,
    version: &str,
    kind: &str,
    deps: &[&str],
    policies: Option<&[&str]>,
) -> PackageManifest {
    let entry = match kind {
        "policy" => serde_json::json!({"policy": "POLICY.md"}),
        "skill" => serde_json::json!({"skill": "SKILL.md"}),
        "agent" => serde_json::json!({"definition": "agent.json"}),
        _ => serde_json::json!({"development": "voice.json"}),
    };
    let mut value = serde_json::json!({
        "id": id,
        "version": version,
        "kind": kind,
        "license": "MIT",
        "authors": [{"name": "Test"}],
        "targets": ["in-graph-agent"],
        "deps": deps,
        "entry": entry,
    });
    if let Some(policies) = policies {
        value["policies"] = serde_json::json!(policies);
    }
    serde_json::from_value(value).unwrap()
}

impl PackageProvider for FakeProvider {
    type Acquired = ();

    fn available_versions(&self, id: &str) -> Result<Vec<Version>, Box<dyn Error>> {
        Ok(self
            .packages
            .get(id)
            .map(|entries| entries.iter().map(|m| ver(&m.version)).collect())
            .unwrap_or_default())
    }

    fn acquire(
        &self,
        id: &str,
        version: &Version,
    ) -> Result<(PackageManifest, ()), Box<dyn Error>> {
        self.acquired.borrow_mut().push(format!("{id}@{version}"));
        let found = self.packages[id]
            .iter()
            .find(|m| ver(&m.version) == *version)
            .cloned()
            .expect("acquired version exists");
        Ok((found, ()))
    }
}

#[test]
fn resolves_transitive_graph() {
    let provider = FakeProvider::new()
        .add("a", "1.0.0", &["b@^0.3"])
        .add("b", "0.3.1", &[]);
    let resolved = resolve_transitive(&["a@^1.0".into()], &provider).unwrap();
    let ids: Vec<_> = resolved.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b"]);
    assert_eq!(resolved[0].version, "1.0.0");
    assert_eq!(resolved[0].dependencies, vec!["b@0.3.1".to_string()]);
    assert_eq!(resolved[1].version, "0.3.1");
}

#[test]
fn satisfied_repeat_requirement_is_not_reacquired() {
    let provider = FakeProvider::new()
        .add("a", "1.0.0", &["c@^1.0"])
        .add("b", "1.0.0", &["c@>=1.0"])
        .add("c", "1.5.0", &[]);
    let resolved = resolve_transitive(&["a@^1.0".into(), "b@^1.0".into()], &provider).unwrap();
    assert_eq!(
        provider
            .acquired
            .borrow()
            .iter()
            .filter(|s| s.starts_with("c@"))
            .count(),
        1
    );
    assert!(resolved.iter().any(|r| r.id == "c" && r.version == "1.5.0"));
}

#[test]
fn incompatible_requirements_conflict() {
    let provider = FakeProvider::new()
        .add("a", "1.0.0", &[])
        .add("a", "2.0.0", &[]);
    let err = match resolve_transitive(&["a@^1.0".into(), "a@^2.0".into()], &provider) {
        Err(err) => err,
        Ok(_) => panic!("expected a dependency conflict"),
    };
    assert!(err.to_string().contains("dependency conflict"));
}

#[test]
fn missing_satisfying_version_errors() {
    let provider = FakeProvider::new().add("a", "1.0.0", &[]);
    let err = match resolve_transitive(&["a@^2.0".into()], &provider) {
        Err(err) => err,
        Ok(_) => panic!("expected a missing-version error"),
    };
    assert!(err.to_string().contains("no available version"));
}

fn policy(id: &str, version: &str) -> PackageManifest {
    manifest(id, version, "policy", &[], None)
}

fn resolve_err<P: PackageProvider>(root: &PackageManifest, provider: &P) -> String {
    match resolve_package(root, provider) {
        Err(err) => err.to_string(),
        Ok(_) => panic!("expected resolution to fail"),
    }
}

#[test]
fn root_policies_resolve_and_lock_as_edges() {
    let provider = FakeProvider::new()
        .add_manifest(policy("fugue.test.rules", "0.1.0"))
        .add_manifest(policy("fugue.test.rules", "0.1.4"));
    let root = manifest(
        "fugue.test.skill",
        "1.0.0",
        "skill",
        &[],
        Some(&["fugue.test.rules@^0.1.0"]),
    );
    let resolved = resolve_package(&root, &provider).unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].id, "fugue.test.rules");
    assert_eq!(resolved[0].version, "0.1.4");
    assert_eq!(resolved[0].manifest.kind, PackageKind::Policy);

    let versions = BTreeMap::from([("fugue.test.rules".to_string(), "0.1.4".to_string())]);
    assert_eq!(
        locked_edges(&root, &versions),
        vec!["fugue.test.rules@0.1.4".to_string()]
    );
}

#[test]
fn a_policy_required_by_many_packages_is_resolved_once() {
    let provider = FakeProvider::new()
        .add_manifest(policy("fugue.test.rules", "0.1.0"))
        .add_manifest(manifest(
            "fugue.test.skill",
            "0.1.0",
            "skill",
            &[],
            Some(&["fugue.test.rules@^0.1.0"]),
        ))
        .add_manifest(manifest(
            "fugue.test.agent",
            "0.1.0",
            "agent",
            &[],
            Some(&["fugue.test.rules@>=0.1"]),
        ));
    let resolved = resolve_transitive(
        &[
            "fugue.test.skill@^0.1.0".into(),
            "fugue.test.agent@^0.1.0".into(),
        ],
        &provider,
    )
    .unwrap();
    let rules: Vec<_> = resolved
        .iter()
        .filter(|r| r.id == "fugue.test.rules")
        .collect();
    assert_eq!(rules.len(), 1);
    assert_eq!(
        provider
            .acquired
            .borrow()
            .iter()
            .filter(|s| s.starts_with("fugue.test.rules@"))
            .count(),
        1
    );
    for requirer in ["fugue.test.skill", "fugue.test.agent"] {
        let entry = resolved.iter().find(|r| r.id == requirer).unwrap();
        assert_eq!(
            entry.dependencies,
            vec!["fugue.test.rules@0.1.0".to_string()]
        );
    }
}

#[test]
fn a_missing_policy_names_the_id() {
    let root = manifest(
        "fugue.test.skill",
        "1.0.0",
        "skill",
        &[],
        Some(&["fugue.test.absent@^1.0.0"]),
    );
    let err = resolve_err(&root, &FakeProvider::new());
    assert!(
        err.contains("fugue.test.absent") && err.contains("policy"),
        "{err}"
    );
}

#[test]
fn an_unsatisfiable_policy_requirement_names_the_id() {
    let provider = FakeProvider::new().add_manifest(policy("fugue.test.rules", "0.1.0"));
    let root = manifest(
        "fugue.test.skill",
        "1.0.0",
        "skill",
        &[],
        Some(&["fugue.test.rules@^0.2.0"]),
    );
    let err = resolve_err(&root, &provider);
    assert!(
        err.contains("fugue.test.rules") && err.contains("^0.2.0"),
        "{err}"
    );
}

#[test]
fn incompatible_policy_ranges_conflict_naming_the_id() {
    let provider = FakeProvider::new()
        .add_manifest(policy("fugue.test.rules", "0.1.0"))
        .add_manifest(policy("fugue.test.rules", "0.2.0"))
        .add_manifest(manifest(
            "fugue.test.agent",
            "0.1.0",
            "agent",
            &[],
            Some(&["fugue.test.rules@^0.2.0"]),
        ));
    let root = manifest(
        "fugue.test.skill",
        "1.0.0",
        "skill",
        &["fugue.test.agent@^0.1.0"],
        Some(&["fugue.test.rules@^0.1.0"]),
    );
    let err = resolve_err(&root, &provider);
    assert!(
        err.contains("policy conflict") && err.contains("fugue.test.rules"),
        "{err}"
    );
}

#[test]
fn a_policy_requirement_on_another_kind_names_the_id_and_kind() {
    for kind in ["development", "skill", "sample-pack"] {
        let provider = FakeProvider::new().add_manifest(manifest(
            "fugue.test.voice",
            "1.0.0",
            kind,
            &[],
            None,
        ));
        let root = manifest(
            "fugue.test.skill",
            "1.0.0",
            "skill",
            &[],
            Some(&["fugue.test.voice@^1.0.0"]),
        );
        let err = resolve_err(&root, &provider);
        assert!(
            err.contains("fugue.test.voice") && err.contains(&format!("`{kind}`")),
            "{err}"
        );
    }
}

#[test]
fn a_root_with_deps_and_policies_locks_both_and_shares_the_policy() {
    let provider = FakeProvider::new()
        .add_manifest(policy("fugue.test.rules", "0.1.0"))
        .add_manifest(policy("fugue.test.rules", "0.1.3"))
        .add_manifest(manifest(
            "fugue.test.skill",
            "1.0.0",
            "skill",
            &[],
            Some(&["fugue.test.rules@^0.1.0"]),
        ));
    let root = manifest(
        "fugue.test.agent",
        "1.0.0",
        "agent",
        &["fugue.test.skill@^1.0"],
        Some(&["fugue.test.rules@^0.1.0"]),
    );
    let resolved = resolve_package(&root, &provider).unwrap();
    let ids: Vec<_> = resolved.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids.iter().filter(|id| **id == "fugue.test.rules").count(),
        1,
        "{ids:?}"
    );
    assert!(ids.contains(&"fugue.test.skill"), "{ids:?}");

    let versions: BTreeMap<_, _> = resolved
        .iter()
        .map(|r| (r.id.clone(), r.version.clone()))
        .collect();
    assert_eq!(versions["fugue.test.rules"], "0.1.3");
    assert_eq!(
        locked_edges(&root, &versions),
        ["fugue.test.skill@1.0.0", "fugue.test.rules@0.1.3"]
    );
}

#[test]
fn two_packages_with_disjoint_policy_ranges_conflict_naming_the_id() {
    let provider = FakeProvider::new()
        .add_manifest(policy("fugue.test.rules", "0.1.0"))
        .add_manifest(policy("fugue.test.rules", "0.2.0"))
        .add_manifest(manifest(
            "fugue.test.skill",
            "1.0.0",
            "skill",
            &[],
            Some(&["fugue.test.rules@^0.1.0"]),
        ))
        .add_manifest(manifest(
            "fugue.test.agent",
            "1.0.0",
            "agent",
            &[],
            Some(&["fugue.test.rules@^0.2.0"]),
        ));
    let err = match resolve_transitive(
        &[
            "fugue.test.skill@^1.0".into(),
            "fugue.test.agent@^1.0".into(),
        ],
        &provider,
    ) {
        Err(err) => err.to_string(),
        Ok(_) => panic!("expected a policy conflict"),
    };
    assert!(err.contains("fugue.test.rules"), "{err}");
}

#[test]
fn a_package_already_chosen_as_a_dependency_must_still_be_a_policy() {
    let provider = FakeProvider::new().add("fugue.test.voice", "1.0.0", &[]);
    let root = manifest(
        "fugue.test.skill",
        "1.0.0",
        "skill",
        &["fugue.test.voice@^1.0.0"],
        Some(&["fugue.test.voice@^1.0.0"]),
    );
    let err = resolve_err(&root, &provider);
    assert!(err.contains("required as a policy"), "{err}");
}
