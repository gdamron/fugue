use super::*;
use serde_json::json;

fn roots() -> (tempfile::TempDir, ContentRoots) {
    let temp = tempfile::tempdir().unwrap();
    let roots = ContentRoots {
        packages: temp.path().join("packs"),
        workspace: temp.path().join("inventions"),
    };
    (temp, roots)
}
fn voice() -> serde_json::Value {
    json!({"title":"Shared voice", "modules":[{"id":"osc", "type":"oscillator"}], "connections":[], "outputs":[{"name":"audio", "from":"osc", "from_port":"audio"}]})
}
fn write(path: &Path, value: &serde_json::Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
fn package(
    roots: &ContentRoots,
    id: &str,
    version: &str,
    document: &serde_json::Value,
) -> ContentRef {
    let root = roots.packages.join(id).join(version);
    write(
        &root.join("fugue.pkg.json"),
        &json!({"id":id,"version":version,"kind":"development","license":"MIT","authors":[{"name":"Test"}],"targets":["external-agent"],"entry":{"development":"voice.json"}}),
    );
    write(&root.join("voice.json"), document);
    ContentRef::Package {
        package: id.into(),
        version: version.into(),
    }
}
fn query(reference: ContentRef) -> ContentDetailQuery {
    ContentDetailQuery {
        schema_version: 1,
        reference,
    }
}

#[test]
fn empty_roots_are_empty_but_unreadable_roots_fail() {
    let (_temp, roots) = roots();
    let mut catalog = ContentCatalog::new(roots.clone(), "session");
    let page = catalog.list(&ContentListQuery::default()).unwrap();
    assert!(page.items.is_empty());
    assert!(page.next_cursor.is_none());
    fs::write(&roots.packages, "not a directory").unwrap();
    assert_eq!(
        catalog.list(&ContentListQuery::default()).unwrap_err().code,
        "catalog_unavailable"
    );
}

#[test]
fn duplicate_titles_are_distinct_exact_versions_and_workspace_never_shadows_packages() {
    let (_temp, roots) = roots();
    package(&roots, "fugue.test.b", "0.2.0", &voice());
    package(&roots, "fugue.test.a", "0.10.0", &voice());
    let exact = package(&roots, "fugue.test.a", "0.2.0", &voice());
    write(&roots.workspace.join("voice.json"), &voice());
    let mut catalog = ContentCatalog::new(roots, "session");
    let page = catalog.list(&ContentListQuery::default()).unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|e| (e.id.as_str(), e.version.as_str()))
            .take(3)
            .collect::<Vec<_>>(),
        vec![
            ("fugue.test.a", "0.2.0"),
            ("fugue.test.a", "0.10.0"),
            ("fugue.test.b", "0.2.0")
        ]
    );
    assert_eq!(page.items.len(), 4);
    let detail = catalog.detail(&query(exact)).unwrap();
    assert_eq!(detail.entry.version, "0.2.0");
}

#[test]
fn external_install_refreshes_and_expires_cursor() {
    let (_temp, roots) = roots();
    package(&roots, "fugue.test.a", "1.0.0", &voice());
    package(&roots, "fugue.test.b", "1.0.0", &voice());
    let mut catalog = ContentCatalog::new(roots.clone(), "session");
    let mut request = ContentListQuery {
        limit: 1,
        ..Default::default()
    };
    let first = catalog.list(&request).unwrap();
    request.cursor = first.next_cursor;
    assert_eq!(catalog.list(&request).unwrap().items[0].id, "fugue.test.b");
    package(&roots, "fugue.test.c", "1.0.0", &voice());
    assert_eq!(catalog.list(&request).unwrap_err().code, "stale_cursor");
    let page = catalog.list(&ContentListQuery::default()).unwrap();
    assert_ne!(page.generation, first.generation);
    assert_eq!(page.items.len(), 3);
}

#[test]
fn workspace_revision_tracks_transitive_files_but_not_unrelated_edits() {
    let (_temp, roots) = roots();
    let mut parent = voice();
    parent["developments"] = json!([{"name":"child", "path":"nested/child.json"}]);
    write(&roots.workspace.join("parent.json"), &parent);
    let mut child = voice();
    child["assets"] = json!({"data":{"path":"data.json"}});
    write(&roots.workspace.join("nested/child.json"), &child);
    write(&roots.workspace.join("nested/data.json"), &json!([1, 2]));
    let mut catalog = ContentCatalog::new(roots.clone(), "session");
    let page = catalog
        .list(&ContentListQuery {
            id: Some("workspace:parent.json".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.items.len(), 1, "{:?}", page.diagnostics);
    let reference = page.items[0].reference.clone();
    write(&roots.workspace.join("unrelated.json"), &json!([7]));
    catalog.detail(&query(reference.clone())).unwrap();
    write(&roots.workspace.join("nested/data.json"), &json!([3]));
    assert_eq!(
        catalog.detail(&query(reference)).unwrap_err().code,
        "stale_reference"
    );
}

#[test]
fn broken_candidates_have_diagnostics_and_cycles_fail_before_import() {
    let (_temp, roots) = roots();
    let mut a = voice();
    a["developments"] = json!([{"name":"child","path":"b.json"}]);
    let mut b = voice();
    b["developments"] = json!([{"name":"child","path":"a.json"}]);
    write(&roots.workspace.join("a.json"), &a);
    write(&roots.workspace.join("b.json"), &b);
    let reference = package(&roots, "fugue.test.missing", "1.0.0", &voice());
    fs::remove_file(roots.packages.join("fugue.test.missing/1.0.0/voice.json")).unwrap();
    let mut catalog = ContentCatalog::new(roots, "session");
    let page = catalog.list(&ContentListQuery::default()).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.diagnostics.len(), 3);
    assert!(catalog.detail(&query(reference)).is_err());
}

#[test]
fn detail_preserves_pad_aliases_and_can_build_without_checkout_paths() {
    let (_temp, roots) = roots();
    let document: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/developments/pad.json")).unwrap();
    let reference = package(&roots, "fugue.instruments.pad", "0.1.0", &document);
    let mut catalog = ContentCatalog::new(roots.clone(), "session");
    let detail = catalog.detail(&query(reference.clone())).unwrap();
    assert_eq!(
        detail
            .interface
            .unwrap()
            .controls
            .iter()
            .map(|c| c.key.as_str())
            .collect::<Vec<_>>(),
        vec!["attack", "release", "warmth", "motion"]
    );
    let definition = roots.load_development(&reference).unwrap();
    assert!(definition
        .source_path
        .unwrap()
        .starts_with(fs::canonicalize(&roots.packages).unwrap()));
    let loaded = roots.load_development(&reference).unwrap();
    crate::InventionBuilder::new(48000).build(loaded).unwrap();
    let authored =
        json!({"modules":[],"connections":[],"developments":[{"name":"pad","ref":reference}]});
    let invention: Invention = serde_json::from_value(authored).unwrap();
    assert_eq!(
        serde_json::to_value(&invention).unwrap()["developments"][0]["ref"],
        serde_json::to_value(reference).unwrap()
    );
}

#[test]
fn strict_references_and_response_limits() {
    assert!(serde_json::from_value::<ContentRef>(
        json!({"package":"a", "version":"1.0.0","workspace_path":"v.json","revision":"sha256:x"})
    )
    .is_err());
    let (_temp, roots) = roots();
    let mut doc = voice();
    doc["description"] = json!("é".repeat(1000));
    write(&roots.workspace.join("voice.json"), &doc);
    let mut catalog = ContentCatalog::new(roots, "session");
    let page = catalog.list(&ContentListQuery::default()).unwrap();
    assert_eq!(page.items[0].summary.len(), 512);
    assert_eq!(
        catalog
            .list(&ContentListQuery {
                limit: 0,
                ..Default::default()
            })
            .unwrap_err()
            .code,
        "invalid_request"
    );
    let mut invalid = page.items[0].reference.clone();
    if let ContentRef::Workspace { revision, .. } = &mut invalid {
        revision.clear();
    }
    assert_eq!(
        catalog.detail(&query(invalid)).unwrap_err().code,
        "invalid_request"
    );
}

#[cfg(unix)]
#[test]
fn symlink_escape_never_yields_a_reference() {
    let (temp, roots) = roots();
    write(&temp.path().join("outside.json"), &voice());
    fs::create_dir_all(&roots.workspace).unwrap();
    std::os::unix::fs::symlink(
        temp.path().join("outside.json"),
        roots.workspace.join("escape.json"),
    )
    .unwrap();
    let mut catalog = ContentCatalog::new(roots, "session");
    let page = catalog.list(&ContentListQuery::default()).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.diagnostics[0].code, "outside_root");
}

#[test]
fn returned_package_reference_imports_and_round_trips_a_complete_invention() {
    let (_temp, roots) = roots();
    let pad =
        serde_json::from_str(include_str!("../../../../examples/developments/pad.json")).unwrap();
    package(&roots, "fugue.instruments.pad", "0.1.0", &pad);
    crate::pkg::audio_asset::with_packs_dir(&roots.packages, || {
        let mut catalog = ContentCatalog::new(roots.clone(), "session");
        let reference = catalog.list(&ContentListQuery::default()).unwrap().items[0]
            .reference
            .clone();
        let definition = json!({"developments":[{"name":"my_pad", "ref":reference}],
            "modules":[{"id":"voice","type":"my_pad"},{"id":"out","type":"dac"}],
            "connections":[{"from":"voice","from_port":"audio","to":"out","to_port":"audio"}]});
        let invention: Invention = serde_json::from_value(definition).unwrap();
        let (runtime, _) = crate::InventionBuilder::new(48000)
            .build(invention)
            .unwrap();
        let running = runtime
            .start_with_backend(crate::NullBackend::new(48000))
            .unwrap();
        let saved = running.document().unwrap();
        drop(running);
        assert_eq!(saved.developments[0].reference.as_ref(), Some(&reference));
        assert!(saved.developments[0].path.is_none());
        let reloaded = Invention::from_json(&saved.to_json().unwrap()).unwrap();
        crate::InventionBuilder::new(48000).build(reloaded).unwrap();
        fs::remove_dir_all(roots.packages.join("fugue.instruments.pad/0.1.0")).unwrap();
        assert_eq!(
            catalog.detail(&query(reference)).unwrap_err().code,
            "content_not_found"
        );
        assert!(crate::InventionBuilder::new(48000)
            .build(saved)
            .err()
            .unwrap()
            .to_string()
            .contains("content_not_found"));
    });
}

#[test]
fn duplicate_import_aliases_and_primitive_collisions_fail() {
    for names in [["voice", "voice"], ["oscillator", "voice"]] {
        let invention = json!({"modules":[],"connections":[],"developments": names.map(|name| json!({"name":name, "definition":voice()}))});
        let error = crate::InventionBuilder::new(48000)
            .build(serde_json::from_value(invention).unwrap())
            .err()
            .unwrap();
        assert!(error.to_string().contains("duplicate_type_name"));
    }
}

#[test]
fn changed_package_payload_is_rejected_against_recorded_integrity() {
    let (_temp, roots) = roots();
    let reference = package(&roots, "fugue.test.voice", "1.0.0", &voice());
    let dir = roots.packages.join("fugue.test.voice/1.0.0");
    let mut lock = Lockfile::new();
    lock.upsert(
        "fugue.test.voice",
        pkg::LockedPackage {
            version: "1.0.0".into(),
            kind: "development".into(),
            source: pkg::LockSource::Local {
                path: String::new(),
            },
            integrity: pkg::compute_integrity(&dir).unwrap(),
            path: dir.clone(),
            dependencies: Vec::new(),
        },
    );
    fs::write(
        roots.packages.parent().unwrap().join(pkg::LOCKFILE_NAME),
        lock.to_bytes().unwrap(),
    )
    .unwrap();
    let mut catalog = ContentCatalog::new(roots.clone(), "session");
    catalog.detail(&query(reference.clone())).unwrap();
    write(
        &dir.join("voice.json"),
        &json!({"modules":[],"connections":[],"outputs":[]}),
    );
    assert_eq!(
        catalog.detail(&query(reference)).unwrap_err().code,
        "integrity_mismatch"
    );
}

#[test]
fn authored_dependencies_use_highest_version_then_an_installed_lock_pin() {
    let (_temp, roots) = roots();
    package(&roots, "fugue.test.child", "1.0.0", &voice());
    package(&roots, "fugue.test.child", "1.2.0", &voice());
    let reference = package(&roots, "fugue.test.parent", "1.0.0", &voice());
    let root = roots.packages.join("fugue.test.parent/1.0.0");
    let path = root.join("fugue.pkg.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["deps"] = json!(["fugue.test.child@^1.0"]);
    write(&path, &manifest);
    let mut catalog = ContentCatalog::new(roots.clone(), "session");
    assert!(catalog
        .detail(&query(reference.clone()))
        .unwrap()
        .dependencies
        .contains(&ContentRef::Package {
            package: "fugue.test.child".into(),
            version: "1.2.0".into()
        }));
    let mut lock = Lockfile::new();
    let child = roots.packages.join("fugue.test.child/1.0.0");
    lock.upsert(
        "fugue.test.child",
        pkg::LockedPackage {
            version: "1.0.0".into(),
            kind: "development".into(),
            source: pkg::LockSource::Local {
                path: String::new(),
            },
            integrity: pkg::compute_integrity(&child).unwrap(),
            path: child,
            dependencies: Vec::new(),
        },
    );
    fs::write(root.join(pkg::LOCKFILE_NAME), lock.to_bytes().unwrap()).unwrap();
    assert!(catalog
        .detail(&query(reference))
        .unwrap()
        .dependencies
        .contains(&ContentRef::Package {
            package: "fugue.test.child".into(),
            version: "1.0.0".into()
        }));
}

#[test]
fn exact_build_metadata_versions_do_not_fall_forward() {
    let (_temp, roots) = roots();
    let reference = package(&roots, "fugue.test.voice", "1.0.0+a", &voice());
    package(&roots, "fugue.test.voice", "1.0.0+z", &voice());
    let mut catalog = ContentCatalog::new(roots.clone(), "session");
    assert_eq!(
        catalog
            .detail(&query(reference.clone()))
            .unwrap()
            .entry
            .version,
        "1.0.0+a"
    );
    fs::remove_dir_all(roots.packages.join("fugue.test.voice/1.0.0+a")).unwrap();
    assert_eq!(
        catalog.detail(&query(reference)).unwrap_err().code,
        "content_not_found"
    );
}

#[test]
fn workspace_reference_dependencies_are_revalidated_independently() {
    let (_temp, roots) = roots();
    write(&roots.workspace.join("child.json"), &voice());
    let mut catalog = ContentCatalog::new(roots.clone(), "session");
    let child_ref = catalog.list(&ContentListQuery::default()).unwrap().items[0]
        .reference
        .clone();
    let mut parent = voice();
    parent["developments"] = json!([{"name":"child","ref":child_ref}]);
    write(&roots.workspace.join("parent.json"), &parent);
    let page = catalog
        .list(&ContentListQuery {
            id: Some("workspace:parent.json".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.items.len(), 1, "{:?}", page.diagnostics);
    let parent_ref = page.items[0].reference.clone();
    catalog.detail(&query(parent_ref.clone())).unwrap();
    let mut changed = voice();
    changed["title"] = json!("Changed voice");
    write(&roots.workspace.join("child.json"), &changed);
    assert_eq!(
        catalog.detail(&query(parent_ref)).unwrap_err().code,
        "stale_reference"
    );
}

#[test]
fn oversized_interfaces_fail_instead_of_truncating_aliases() {
    let (_temp, roots) = roots();
    let mut large = voice();
    large["controls"] = json!((0..6000)
        .map(|i| json!({"key":format!("control{i}"),"module":"osc","control":"frequency"}))
        .collect::<Vec<_>>());
    let reference = package(&roots, "fugue.test.large", "1.0.0", &large);
    let mut catalog = ContentCatalog::new(roots, "session");
    assert_eq!(
        catalog
            .list(&ContentListQuery::default())
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        catalog.detail(&query(reference)).unwrap_err().code,
        "response_too_large"
    );
}
