use super::*;
use serde_json::json;

fn agent_manifest(definition: &str) -> PackageManifest {
    serde_json::from_value(json!({
        "id": "fugue.test.agent",
        "version": "0.1.0",
        "kind": "agent",
        "license": "MIT",
        "authors": [{"name": "Test"}],
        "targets": ["in-graph-agent"],
        "entry": {"definition": definition},
    }))
    .unwrap()
}

fn package_with(definition: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("agent.json"), definition).unwrap();
    dir
}

fn check(definition: Value) -> Result<AgentDefinition, AgentPackageError> {
    let dir = package_with(&definition.to_string());
    check_agent_package(dir.path(), &agent_manifest("agent.json"))
}

#[test]
fn accepts_a_persona_with_tools_and_a_model() {
    let definition = check(json!({
        "system_prompt": "You are a pulse agent.",
        "default_tools": ["fugue.set_control", "fugue.swap_module"],
        "default_model": "a-model",
        "id": "fugue.test.agent",
        "version": "0.1.0",
    }))
    .unwrap();
    assert_eq!(definition.system_prompt, "You are a pulse agent.");
    assert_eq!(
        definition.default_tools,
        ["fugue.set_control", "fugue.swap_module"]
    );
    assert_eq!(definition.default_model.as_deref(), Some("a-model"));
}

#[test]
fn tools_and_model_are_optional() {
    let definition = check(json!({"system_prompt": "Persona."})).unwrap();
    assert!(definition.default_tools.is_empty());
    assert_eq!(definition.default_model, None);
}

#[test]
fn refuses_unknown_fields_naming_them() {
    for field in ["skills", "policies", "kind", "temperature"] {
        let err = check(json!({"system_prompt": "Persona.", field: []})).unwrap_err();
        assert!(
            matches!(&err, AgentPackageError::UnknownField { field: f, .. } if f == field),
            "{field}: {err:?}"
        );
        assert!(err.to_string().contains(&format!("`{field}`")), "{err}");
    }
}

#[test]
fn refuses_a_missing_or_empty_prompt_naming_the_field() {
    for definition in [
        json!({}),
        json!({"system_prompt": ""}),
        json!({"system_prompt": "  \n"}),
        json!({"default_tools": []}),
    ] {
        let err = check(definition.clone()).unwrap_err();
        assert!(
            matches!(
                err,
                AgentPackageError::MissingField {
                    field: "system_prompt",
                    ..
                }
            ),
            "{definition}: {err:?}"
        );
        assert!(err.to_string().contains("`system_prompt`"), "{err}");
    }
}

#[test]
fn refuses_wrong_types_naming_the_field() {
    for (definition, field) in [
        (json!({"system_prompt": 3}), "system_prompt"),
        (
            json!({"system_prompt": "P", "default_tools": "fugue.set_control"}),
            "default_tools",
        ),
        (
            json!({"system_prompt": "P", "default_tools": [1]}),
            "default_tools",
        ),
        (
            json!({"system_prompt": "P", "default_tools": [""]}),
            "default_tools",
        ),
        (
            json!({"system_prompt": "P", "default_model": 4}),
            "default_model",
        ),
    ] {
        let err = check(definition.clone()).unwrap_err();
        assert!(err.to_string().contains(&format!("`{field}`")), "{err}");
    }
    let err = check(json!({"system_prompt": "P", "default_model": ""})).unwrap_err();
    assert!(matches!(
        err,
        AgentPackageError::MissingField {
            field: "default_model",
            ..
        }
    ));
}

#[test]
fn an_id_or_version_must_equal_the_manifests() {
    for (field, value) in [("id", "fugue.test.other"), ("version", "0.2.0")] {
        let err = check(json!({"system_prompt": "P", field: value})).unwrap_err();
        assert!(
            matches!(&err, AgentPackageError::Mismatch { field: f, .. } if *f == field),
            "{err:?}"
        );
        assert!(
            err.to_string().contains(&format!("`{field}`")) && err.to_string().contains(value),
            "{err}"
        );
    }
}

#[test]
fn refuses_a_missing_definition_naming_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let err = check_agent_package(dir.path(), &agent_manifest("agent.json")).unwrap_err();
    assert!(matches!(
        err,
        AgentPackageError::Definition(PackageFileError::Missing(_))
    ));
    assert!(err.to_string().contains("agent.json"), "{err}");
}

#[test]
fn refuses_a_definition_outside_the_package_naming_the_path() {
    let dir = package_with("{}");
    for entry in ["../agent.json", "/agent.json", ".git/agent.json"] {
        let err = check_agent_package(dir.path(), &agent_manifest(entry)).unwrap_err();
        assert!(
            matches!(
                err,
                AgentPackageError::Definition(PackageFileError::OutsidePackage(_))
            ),
            "{entry}: {err:?}"
        );
        assert!(err.to_string().contains(entry), "{err}");
    }
}

#[cfg(unix)]
#[test]
fn refuses_a_linked_definition_naming_the_path() {
    let outside = package_with(r#"{"system_prompt": "P"}"#);
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("agent.json"),
        dir.path().join("agent.json"),
    )
    .unwrap();
    let err = check_agent_package(dir.path(), &agent_manifest("agent.json")).unwrap_err();
    assert!(matches!(
        err,
        AgentPackageError::Definition(PackageFileError::Linked(_))
    ));
    assert!(err.to_string().contains("agent.json"), "{err}");
}

#[test]
fn refuses_invalid_json_naming_the_path() {
    for text in ["{", "[]", "\"prompt\""] {
        let dir = package_with(text);
        let err = check_agent_package(dir.path(), &agent_manifest("agent.json")).unwrap_err();
        assert!(
            matches!(err, AgentPackageError::InvalidJson { .. }),
            "{text}: {err:?}"
        );
        assert!(err.to_string().contains("agent.json"), "{err}");
    }
}

#[test]
fn refuses_a_package_of_another_kind() {
    let mut manifest = agent_manifest("agent.json");
    manifest.kind = crate::pkg::PackageKind::Skill;
    manifest.entry = EntrySpec::Skill {
        skill: "SKILL.md".into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let err = check_agent_package(dir.path(), &manifest).unwrap_err();
    assert!(matches!(err, AgentPackageError::NotAgentPackage { .. }));
}
