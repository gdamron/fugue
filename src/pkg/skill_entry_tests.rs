use super::*;

const SKILL: &str =
    "---\nname: pulse-study\ndescription: Shape a pulse into a phrase, then return \
                     to the opening idea.\nallowed-tools: Read\n---\n\
                     # Pulse\n\nFollow the [rules](policies/fugue.test.rules.md).\n";
const INVENTION: &str = r#"{"modules": [], "connections": []}"#;

fn manifest(extra: serde_json::Value) -> PackageManifest {
    let mut value = serde_json::json!({
        "id": "fugue.test.skill",
        "version": "0.1.0",
        "kind": "skill",
        "license": "CC0-1.0",
        "authors": [{"name": "Test"}],
        "targets": ["external-agent"],
        "policies": ["fugue.test.rules@^0.1.0"],
        "entry": {"skill": "SKILL.md"},
    });
    for (key, field) in extra.as_object().unwrap() {
        value[key] = field.clone();
    }
    serde_json::from_value(value).unwrap()
}

fn package(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn check(
    files: &[(&str, &str)],
    extra: serde_json::Value,
) -> Result<SkillCheck, SkillPackageError> {
    let dir = package(files);
    check_skill_package(dir.path(), &manifest(extra))
}

#[test]
fn accepts_a_skill_with_declared_policies_and_examples() {
    let checked = check(
        &[
            ("SKILL.md", SKILL),
            ("examples/a.invention.json", INVENTION),
            (
                "examples/voice.json",
                r#"{"modules": [{"id": "osc", "type": "oscillator"}], "connections": [], "inputs": [],
                    "outputs": [{"name": "audio", "from": "osc", "from_port": "audio"}],
                    "controls": [{"name": "pitch", "module": "osc", "control": "frequency"}]}"#,
            ),
        ],
        serde_json::json!({"examples": ["examples/a.invention.json", "./examples/voice.json"]}),
    )
    .unwrap();
    assert_eq!(checked.name, "pulse-study");
    assert!(checked.entry.ends_with("SKILL.md"), "{:?}", checked.entry);
    assert!(checked.warnings.is_empty(), "{:?}", checked.warnings);
    check(&[("SKILL.md", SKILL)], serde_json::json!({"examples": []})).unwrap();
}

#[test]
fn refuses_entry_path_problems_naming_the_path() {
    let nested = check(
        &[],
        serde_json::json!({"entry": {"skill": "docs/SKILL.md"}}),
    )
    .unwrap_err();
    assert!(
        matches!(
            nested,
            SkillPackageError::Entry(PackageFileError::Missing(_))
        ),
        "{nested}"
    );
    assert!(nested.to_string().contains("docs/SKILL.md"), "{nested}");

    for entry in ["../SKILL.md", "/tmp/SKILL.md", ".git/SKILL.md"] {
        let err = check(&[], serde_json::json!({"entry": {"skill": entry}})).unwrap_err();
        assert!(
            matches!(
                err,
                SkillPackageError::Entry(PackageFileError::OutsidePackage(_))
            ),
            "{entry}: {err}"
        );
        assert!(err.to_string().contains(entry), "{err}");
    }

    let err = check(
        &[("skill.md", SKILL)],
        serde_json::json!({"entry": {"skill": "skill.md"}}),
    )
    .unwrap_err();
    assert!(err.to_string().contains("must be a SKILL.md file"), "{err}");
}

#[cfg(unix)]
#[test]
fn refuses_a_linked_entry() {
    let outside = package(&[("SKILL.md", SKILL)]);
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path().join("SKILL.md"), dir.path().join("SKILL.md"))
        .unwrap();
    let err = check_skill_package(dir.path(), &manifest(serde_json::json!({}))).unwrap_err();
    assert!(
        matches!(err, SkillPackageError::Entry(PackageFileError::Linked(_))),
        "{err}"
    );
    assert!(err.to_string().contains("SKILL.md"), "{err}");

    let dir = package(&[("real/SKILL.md", SKILL)]);
    std::os::unix::fs::symlink("real", dir.path().join("linked")).unwrap();
    let through = manifest(serde_json::json!({"entry": {"skill": "linked/SKILL.md"}}));
    let err = check_skill_package(dir.path(), &through).unwrap_err();
    assert!(
        matches!(err, SkillPackageError::Entry(PackageFileError::Linked(_))),
        "{err}"
    );
    assert!(err.to_string().contains("linked"), "{err}");
}

#[test]
fn refuses_a_missing_or_unclosed_frontmatter_block() {
    for text in [
        "# Pulse\n",
        "---\nname: a\ndescription: b\n# Pulse\n",
        "\n---\nname: a\ndescription: b\n---\n",
        "",
    ] {
        let err = check(&[("SKILL.md", text)], serde_json::json!({})).unwrap_err();
        assert!(
            matches!(err, SkillPackageError::NoFrontmatter(_)),
            "{text:?}: {err}"
        );
        assert!(err.to_string().contains("SKILL.md"), "{err}");
    }
}

#[test]
fn refuses_missing_or_empty_fields_naming_the_field() {
    for (text, field) in [
        ("---\ndescription: d\n---\n", "name"),
        ("---\nname: \"\"\ndescription: d\n---\n", "name"),
        ("---\nname:\ndescription: d\n---\n", "name"),
        ("---\nname: a\ndescription: \"\"\n---\n", "description"),
        ("---\nname: a\n---\n", "description"),
        ("---\nname: a\ndescription:   \n---\n", "description"),
        ("---\nname: null\ndescription: d\n---\n", "name"),
        (
            "---\nname: a\ndescription:\n  # a comment\n---\n",
            "description",
        ),
    ] {
        let err = check(&[("SKILL.md", text)], serde_json::json!({})).unwrap_err();
        assert_eq!(
            err,
            SkillPackageError::MissingField {
                path: match &err {
                    SkillPackageError::MissingField { path, .. } => path.clone(),
                    other => panic!("{text:?}: {other}"),
                },
                field,
            }
        );
        assert!(err.to_string().contains(&format!("`{field}`")), "{err}");
    }
}

#[test]
fn refuses_unreadable_or_non_text_frontmatter() {
    let bad = check(
        &[("SKILL.md", "---\nname: a\ndescription: [\n---\n")],
        serde_json::json!({}),
    )
    .unwrap_err();
    assert!(
        matches!(bad, SkillPackageError::BadFrontmatter { .. }),
        "{bad}"
    );
    assert!(bad.to_string().contains("SKILL.md"), "{bad}");

    for (text, field) in [
        ("---\nname: 12\ndescription: d\n---\n", "name"),
        ("---\nname: a\ndescription: [d]\n---\n", "description"),
    ] {
        let err = check(&[("SKILL.md", text)], serde_json::json!({})).unwrap_err();
        assert!(
            matches!(err, SkillPackageError::FieldNotText { .. }),
            "{text:?}: {err}"
        );
        assert!(err.to_string().contains(&format!("`{field}`")), "{err}");
    }
}

#[test]
fn refuses_a_name_that_is_not_host_safe() {
    let long = "a".repeat(65);
    for name in [
        "Pulse",
        "pulse_study",
        "pulse study",
        "pulse.study",
        "pulse/study",
        "caf\u{e9}",
        long.as_str(),
    ] {
        let text = format!("---\nname: {name}\ndescription: d\n---\n");
        let err = check(&[("SKILL.md", &text)], serde_json::json!({})).unwrap_err();
        assert!(
            matches!(err, SkillPackageError::NameNotHostSafe { .. }),
            "{name}: {err}"
        );
        assert!(err.to_string().contains("`name`"), "{err}");
        assert!(!is_host_safe_name(name), "{name}");
    }
    assert!(!is_host_safe_name(""));
    assert!(is_host_safe_name(&"a".repeat(64)));
    for name in ["a", "pulse-2", "3d-pulse"] {
        assert!(is_host_safe_name(name), "{name}");
    }
}

#[test]
fn refuses_bad_examples_naming_the_path() {
    let cases: &[(&[(&str, &str)], &str)] = &[
        (&[], "examples/missing.json"),
        (&[], "../outside.json"),
        (
            &[("examples/notes.json", r#"{"title": "no modules"}"#)],
            "examples/notes.json",
        ),
        (&[("examples/bad.json", "not json")], "examples/bad.json"),
        (
            &[("examples/bad.json", r#"{"modules": 5, "connections": []}"#)],
            "examples/bad.json",
        ),
        (&[("examples/bad.json", "[]")], "examples/bad.json"),
    ];
    for (files, example) in cases {
        let mut files = files.to_vec();
        files.extend([("SKILL.md", SKILL), ("examples/good.json", INVENTION)]);
        let examples = serde_json::json!({"examples": ["examples/good.json", example]});
        let err = check(&files, examples).unwrap_err();
        assert!(!err.to_string().contains("good.json"), "{example}: {err}");
        assert!(
            matches!(
                err,
                SkillPackageError::Example(_) | SkillPackageError::InvalidExample { .. }
            ),
            "{example}: {err}"
        );
        if example.starts_with("../") {
            assert!(
                matches!(
                    err,
                    SkillPackageError::Example(PackageFileError::OutsidePackage(_))
                ),
                "{example}: {err}"
            );
        }
        let named = example.trim_start_matches("../");
        assert!(err.to_string().contains(named), "{example}: {err}");
    }
}

#[test]
fn warns_on_undeclared_policy_links_without_failing() {
    let text = "---\nname: a\ndescription: d\n---\n\
                Use [rules](policies/fugue.test.rules.md) and [mix](./policies/fugue.test.mix.md#top).\n\
                Again [mix](./policies/fugue.test.mix.md#top).\n\
                [ref]: policies/fugue.test.ref.md\n\
                ```\n[code](policies/fugue.test.code.md)\n```\n\
                Not a policy: [doc](docs/fugue.test.doc.md) or [site](https://example.com/guide).\n";
    let checked = check(&[("SKILL.md", text)], serde_json::json!({})).unwrap();
    let links: Vec<String> = checked.warnings.iter().map(ToString::to_string).collect();
    assert_eq!(links.len(), 2, "{links:?}");
    assert!(
        links[0].contains("./policies/fugue.test.mix.md#top"),
        "{links:?}"
    );
    assert!(links[1].contains("policies/fugue.test.ref.md"), "{links:?}");

    let none_listed = check(
        &[("SKILL.md", SKILL)],
        serde_json::json!({"policies": null}),
    )
    .unwrap();
    assert_eq!(none_listed.warnings.len(), 1, "{:?}", none_listed.warnings);
    assert!(none_listed.warnings[0]
        .to_string()
        .contains("policies/fugue.test.rules.md"));
}

#[test]
fn refuses_a_package_of_another_kind() {
    let mut other = manifest(serde_json::json!({}));
    other.kind = crate::pkg::PackageKind::Policy;
    other.entry = EntrySpec::Policy {
        policy: "POLICY.md".into(),
    };
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        check_skill_package(dir.path(), &other),
        Err(SkillPackageError::NotSkillPackage { .. })
    ));
}

#[test]
fn a_compressed_example_is_checked_and_a_corrupt_one_refused() {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(INVENTION.as_bytes()).unwrap();
    let packed = encoder.finish().unwrap();
    let dir = package(&[("SKILL.md", SKILL)]);
    std::fs::create_dir_all(dir.path().join("examples")).unwrap();
    std::fs::write(dir.path().join("examples/piece.fyug"), &packed).unwrap();
    let examples = serde_json::json!({"examples": ["examples/piece.fyug"]});
    check_skill_package(dir.path(), &manifest(examples.clone())).unwrap();

    std::fs::write(
        dir.path().join("examples/piece.fyug"),
        &packed[..packed.len() / 2],
    )
    .unwrap();
    let err = check_skill_package(dir.path(), &manifest(examples)).unwrap_err();
    assert!(matches!(err, SkillPackageError::Unreadable { .. }), "{err}");
    assert!(err.to_string().contains("piece.fyug"), "{err}");
    assert!(err.to_string().contains("cannot be decompressed"), "{err}");
}
