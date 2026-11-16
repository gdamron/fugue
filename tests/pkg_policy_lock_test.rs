//! Locking a `policy` package: the lock verifies a policy's text by its
//! content hash, and graphs and lockfiles without policies are unchanged.

use std::collections::BTreeMap;
use std::fs;

use fugue::pkg::{dependency_edges, locked_edges, parse_str};
use fugue::{LockSource, LockedPackage, Lockfile};

const POLICY_ID: &str = "fugue.example.mixing-policy";
const POLICY: &str = include_str!("../src/pkg/examples/policy.json");
const SKILL: &str = include_str!("../src/pkg/examples/skill.json");

#[test]
fn a_locked_policy_is_verified_by_its_content_hash() {
    let packs = tempfile::tempdir().unwrap();
    let dir = packs.path().join(POLICY_ID).join("0.1.0");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("POLICY.md"),
        "# Rules\n\nKeep the master under -3 dBFS.\n",
    )
    .unwrap();
    fs::write(dir.join("fugue.pkg.json"), POLICY).unwrap();

    let mut lock = Lockfile::new();
    lock.upsert(
        POLICY_ID,
        LockedPackage {
            version: "0.1.0".into(),
            kind: "policy".into(),
            source: LockSource::Local {
                path: dir.display().to_string(),
            },
            integrity: fugue::compute_integrity(&dir).unwrap(),
            path: dir.clone(),
            dependencies: Vec::new(),
        },
    );
    lock.verify(packs.path())
        .unwrap_or_else(|e| panic!("an untouched policy should verify: {e}"));

    fs::write(dir.join("POLICY.md"), "# Rules\n\nLoud is fine.\n").unwrap();
    let err = lock
        .verify(packs.path())
        .expect_err("an edited policy must not verify");
    assert!(
        err.to_string().contains(POLICY_ID),
        "the failure names the policy: {err}"
    );
}

#[test]
fn without_policies_the_lock_edges_are_the_dependency_edges() {
    let mut skill: serde_json::Value = serde_json::from_str(SKILL).unwrap();
    skill.as_object_mut().unwrap().remove("policies");
    skill["deps"] = serde_json::json!(["fugue.example.helper@^0.3"]);
    let manifest = parse_str(&skill.to_string()).unwrap();
    let versions = BTreeMap::from([("fugue.example.helper".to_string(), "0.3.1".to_string())]);
    let edges = locked_edges(&manifest, &versions);
    assert_eq!(edges, dependency_edges(&manifest.deps, &versions));
    assert_eq!(edges, ["fugue.example.helper@0.3.1"]);
}

#[test]
fn existing_lockfiles_stay_valid() {
    let v2 = br#"{
  "version": 2,
  "roots": ["fugue.demo.top-skill"],
  "packages": {
    "fugue.demo.helper-skill": {
      "version": "0.3.1",
      "kind": "skill",
      "source": { "registry": { "id": "fugue.demo.helper-skill", "version": "0.3.1" } },
      "integrity": "sha256:abc123",
      "path": "/packs/fugue.demo.helper-skill/0.3.1"
    },
    "fugue.demo.top-skill": {
      "version": "1.0.0",
      "kind": "skill",
      "source": { "registry": { "id": "fugue.demo.top-skill", "version": "1.0.0" } },
      "integrity": "sha256:def456",
      "path": "/packs/fugue.demo.top-skill/1.0.0",
      "dependencies": ["fugue.demo.helper-skill@0.3.1"]
    }
  }
}"#;
    let lock = Lockfile::from_slice(v2).expect("a v2 lockfile still parses");
    assert_eq!(lock.packages.len(), 2);
    assert_eq!(lock.packages["fugue.demo.top-skill"].kind, "skill");
    assert_eq!(
        lock.packages["fugue.demo.top-skill"].dependencies,
        ["fugue.demo.helper-skill@0.3.1"]
    );
    let rewritten = Lockfile::from_slice(&lock.to_bytes().unwrap()).unwrap();
    assert_eq!(lock, rewritten, "writing it back changes nothing");

    let v1 = br#"{ "version": 1, "packages": { "fugue.demo.old": {
        "version": "1.0.0", "kind": "module", "source": "local:/x", "path": "/x" } } }"#;
    let upgraded = Lockfile::from_slice(v1).expect("a v1 lockfile still upgrades");
    assert_eq!(upgraded.packages["fugue.demo.old"].version, "1.0.0");
}
