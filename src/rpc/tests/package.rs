use super::*;
use crate::pkg::content::{receipt_path, ContentReceipt, ContentRef};
use crate::pkg::{LockSource, LockedPackage, PackageKind};
use crate::ModuleRegistry;
use std::path::Path;

#[test]
fn built_in_packages_list_registry_types() {
    let registry = ModuleRegistry::default();
    let packages = PackageList::built_in(&registry);
    assert_eq!(packages.packages.len(), 1);
    assert_eq!(packages.packages[0].source, PackageSource::BuiltIn);
    assert!(packages.packages[0]
        .module_types
        .contains(&"oscillator".to_string()));
}

fn install(dir: &Path, id: &str, version: &str, kind: &str, description: &str) {
    let root = dir.join(id).join(version);
    std::fs::create_dir_all(&root).unwrap();
    let entry = match kind {
        "development" => r#"{"development":"voice.json"}"#,
        _ => r#"{"wasm":"lib.wasm"}"#,
    };
    std::fs::write(
        root.join("fugue.pkg.json"),
        format!(
            r#"{{"id":"{id}","version":"{version}","kind":"{kind}","license":"MIT",
            "authors":[{{"name":"T"}}],"targets":["in-graph-agent"],
            "description":{description:?},"deps":["fugue.test.dep@^1"],"entry":{entry}}}"#
        ),
    )
    .unwrap();
}

fn list(dir: &Path, query: &PackageListQuery) -> PackageList {
    PackageList::local(&ModuleRegistry::default(), dir, query).unwrap()
}

#[test]
fn local_list_is_sorted_compact_and_offline() {
    let temp = tempfile::tempdir().unwrap();
    install(
        temp.path(),
        "fugue.test.voice",
        "1.10.0",
        "development",
        "Pad",
    );
    install(
        temp.path(),
        "fugue.test.voice",
        "1.2.0",
        "development",
        "Pad",
    );
    install(temp.path(), "acme.test.mod", "0.1.0", "module", "");

    let page = list(temp.path(), &PackageListQuery::default());
    let ids: Vec<_> = page
        .packages
        .iter()
        .map(|p| format!("{}@{}", p.id, p.version))
        .collect();
    assert_eq!(ids[0], "acme.test.mod@0.1.0");
    assert!(ids[1].starts_with("builtin@"));
    assert_eq!(
        &ids[2..],
        ["fugue.test.voice@1.2.0", "fugue.test.voice@1.10.0"]
    );
    let voice = &page.packages[2];
    assert_eq!(voice.kind, PackageKind::Development);
    assert_eq!(voice.source, PackageSource::Installed);
    assert_eq!(
        voice.content_ref,
        Some(ContentRef::Package {
            package: "fugue.test.voice".into(),
            version: "1.2.0".into()
        })
    );
    assert!(page.packages.iter().all(|p| p.module_types.is_empty()));
    assert!(page.packages.iter().all(|p| p.dependencies.is_empty()));
    assert_eq!(page.packages[0].content_ref, None);

    let detail = list(
        temp.path(),
        &PackageListQuery {
            id: Some("builtin".into()),
            detail: true,
            ..Default::default()
        },
    );
    assert_eq!(detail.packages.len(), 1);
    assert!(!detail.packages[0].module_types.is_empty());
}

#[test]
fn local_list_filters_pages_and_expires_cursors() {
    let temp = tempfile::tempdir().unwrap();
    for minor in 0..3 {
        install(
            temp.path(),
            "fugue.test.voice",
            &format!("1.{minor}.0"),
            "development",
            "",
        );
    }
    let query = PackageListQuery {
        kind: Some(PackageKind::Development),
        limit: 2,
        ..Default::default()
    };
    let first = list(temp.path(), &query);
    assert_eq!(first.packages.len(), 2);
    let cursor = first.next_cursor.clone().unwrap();
    let second = list(
        temp.path(),
        &PackageListQuery {
            cursor: Some(cursor.clone()),
            ..query.clone()
        },
    );
    assert_eq!(second.packages.len(), 1);
    assert_eq!(second.packages[0].version, "1.2.0");
    assert_eq!(second.next_cursor, None);

    install(temp.path(), "fugue.test.voice", "2.0.0", "development", "");
    let error = PackageList::local(
        &ModuleRegistry::default(),
        temp.path(),
        &PackageListQuery {
            cursor: Some(cursor),
            ..query
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "stale_cursor");
}

#[test]
fn local_list_reports_bundled_provenance_and_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    install(temp.path(), "fugue.test.voice", "1.0.0", "development", "");
    let receipt = ContentReceipt {
        package: LockedPackage {
            version: "1.0.0".into(),
            kind: "development".into(),
            source: LockSource::Local { path: "/x".into() },
            integrity: "sha256:0".into(),
            path: temp.path().join("fugue.test.voice/1.0.0"),
            dependencies: Vec::new(),
        },
        bundled: true,
    };
    let path = receipt_path(temp.path(), "fugue.test.voice", "1.0.0");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    std::fs::create_dir_all(temp.path().join("broken/1.0.0")).unwrap();
    std::fs::write(temp.path().join("broken/1.0.0/fugue.pkg.json"), "{").unwrap();
    std::fs::create_dir_all(temp.path().join("fugue.test.voice/not-semver")).unwrap();

    let page = list(
        temp.path(),
        &PackageListQuery {
            source: Some(PackageSource::Bundled),
            ..Default::default()
        },
    );
    assert_eq!(page.packages.len(), 1);
    assert_eq!(page.packages[0].id, "fugue.test.voice");
    assert_eq!(page.diagnostics.len(), 1);
    assert_eq!(page.diagnostics[0].code, "invalid_content");
}

#[test]
fn local_list_bounds_summaries_and_rejects_bad_queries() {
    let temp = tempfile::tempdir().unwrap();
    install(
        temp.path(),
        "fugue.test.voice",
        "1.0.0",
        "development",
        &"é".repeat(400),
    );
    let page = list(temp.path(), &PackageListQuery::default());
    let voice = page
        .packages
        .iter()
        .find(|p| p.id == "fugue.test.voice")
        .unwrap();
    assert!(voice.summary.len() <= MAX_PACKAGE_SUMMARY_BYTES);

    for query in [
        PackageListQuery {
            limit: 0,
            ..Default::default()
        },
        PackageListQuery {
            schema_version: 2,
            ..Default::default()
        },
    ] {
        let error =
            PackageList::local(&ModuleRegistry::default(), temp.path(), &query).unwrap_err();
        assert_eq!(error.code, "invalid_request");
    }
    let missing = temp.path().join("absent");
    assert_eq!(
        list(&missing, &PackageListQuery::default()).packages.len(),
        1
    );
}

#[test]
fn installed_entry_and_wire_shapes() {
    let temp = tempfile::tempdir().unwrap();
    install(
        temp.path(),
        "fugue.test.voice",
        "1.0.0",
        "development",
        "Pad",
    );
    let entry = PackageInfo::installed(temp.path(), "fugue.test.voice", "1.0.0").unwrap();
    assert_eq!(entry.dependencies, ["fugue.test.dep@^1"]);
    let missing = PackageInfo::installed(temp.path(), "fugue.test.voice", "9.0.0").unwrap_err();
    assert_eq!(missing.code, "content_not_found");

    let json = serde_json::to_value(entry.compact()).unwrap();
    assert_eq!(json["ref"]["package"], "fugue.test.voice");
    assert_eq!(json["source"], "installed");
    assert!(json.get("module_types").is_none());

    let command: RpcCommand =
        serde_json::from_value(serde_json::json!({"command": "list_packages"})).unwrap();
    assert_eq!(
        command,
        RpcCommand::ListPackages {
            query: PackageListQuery::default()
        }
    );
    let request: PackageInstallRequest =
        serde_json::from_value(serde_json::json!({"package": "local:/tmp/p"})).unwrap();
    assert_eq!(request.version, None);
}
