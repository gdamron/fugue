//! A pack repository's source checkout (`<packs>/<id>/fugue.pkg.json`) used
//! as the package cache: exact pins still hold, and identity is unchanged.

use super::*;
use serde_json::json;

fn write(path: &Path, value: &serde_json::Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn voice() -> serde_json::Value {
    json!({"title":"Piano", "modules":[{"id":"osc", "type":"oscillator"}], "connections":[], "outputs":[{"name":"audio", "from":"osc", "from_port":"audio"}]})
}

/// A checkout with a piano development at 2026.10.2 and a starter invention
/// that pins `pin` and refers to the piano at `reference_version`.
fn checkout(pin: &str, reference_version: &str) -> (tempfile::TempDir, ContentRoots) {
    let temp = tempfile::tempdir().unwrap();
    let roots = ContentRoots {
        packages: temp.path().join("packs"),
        workspace: temp.path().join("inventions"),
    };
    let piano = roots.packages.join("fugue.instruments.piano");
    write(
        &piano.join("fugue.pkg.json"),
        &json!({"id":"fugue.instruments.piano","version":"2026.10.2","kind":"development","license":"MIT","authors":[{"name":"Test"}],"targets":["external-agent"],"entry":{"development":"voice.json"}}),
    );
    write(&piano.join("voice.json"), &voice());
    let starter = roots.packages.join("fugue.starter.study");
    write(
        &starter.join("fugue.pkg.json"),
        &json!({"id":"fugue.starter.study","version":"2026.10.6","kind":"invention","license":"MIT","authors":[{"name":"Test"}],"targets":["external-agent"],"deps":[format!("fugue.instruments.piano@{pin}")],"entry":{"invention":"invention.json"}}),
    );
    write(
        &starter.join("invention.json"),
        &json!({
            "title":"Study",
            "developments":[{"name":"piano","ref":{"package":"fugue.instruments.piano","version":reference_version}}],
            "modules":[{"id":"voice","type":"piano"},{"id":"out","type":"dac"}],
            "connections":[{"from":"voice","from_port":"audio","to":"out","to_port":"audio"}]
        }),
    );
    (temp, roots)
}

fn starter() -> ContentRef {
    ContentRef::Package {
        package: "fugue.starter.study".into(),
        version: "2026.10.6".into(),
    }
}

#[test]
fn source_checkout_lists_and_loads_exact_pins_without_staging() {
    let (_temp, roots) = checkout("=2026.10.2", "2026.10.2");
    let mut catalog = ContentCatalog::new(roots.clone(), "session");
    let page = catalog.list(&ContentListQuery::default()).unwrap();
    let listed: Vec<_> = page
        .items
        .iter()
        .map(|e| (e.id.as_str(), e.version.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            ("fugue.instruments.piano", "2026.10.2"),
            ("fugue.starter.study", "2026.10.6")
        ],
        "{:?}",
        page.diagnostics
    );
    let loaded = catalog
        .load_invention(&ContentDetailQuery {
            schema_version: 1,
            reference: starter(),
        })
        .unwrap();
    crate::pkg::audio_asset::with_packs_dir(&roots.packages, || {
        crate::InventionBuilder::new(48_000).build(loaded).unwrap();
        // The harness path: the invention file itself, loaded and built.
        let path = roots
            .packages
            .join("fugue.starter.study/invention.json")
            .to_string_lossy()
            .into_owned();
        let document = Invention::from_file(&path).unwrap();
        crate::InventionBuilder::new(48_000)
            .build(document)
            .unwrap();
    });
}

#[test]
fn source_checkout_refuses_a_pin_its_manifest_version_does_not_satisfy() {
    let (_temp, roots) = checkout("=2026.10.1", "2026.10.2");
    let error = roots.load_invention(&starter()).unwrap_err();
    assert_eq!(error.code, "dependency_not_found");
    assert!(
        error.message.contains("is version 2026.10.2")
            && error.message.contains("does not satisfy =2026.10.1"),
        "{}",
        error.message
    );

    let (_temp, roots) = checkout("=2026.10.2", "2026.10.1");
    let error = roots
        .load_development(&ContentRef::Package {
            package: "fugue.instruments.piano".into(),
            version: "2026.10.1".into(),
        })
        .unwrap_err();
    assert_eq!(error.code, "content_not_found");
}

#[test]
fn mixed_layouts_are_refused_not_chosen() {
    let (_temp, roots) = checkout("=2026.10.2", "2026.10.2");
    let piano = roots.packages.join("fugue.instruments.piano");
    fs::create_dir_all(piano.join("2026.10.2")).unwrap();
    for file in ["fugue.pkg.json", "voice.json"] {
        fs::copy(piano.join(file), piano.join("2026.10.2").join(file)).unwrap();
    }
    let error = roots.load_invention(&starter()).unwrap_err();
    assert!(
        error.message.contains("both installed versions"),
        "{}",
        error.message
    );
}

#[test]
fn lockfile_verification_hashes_the_checkout_bytes() {
    let (_temp, roots) = checkout("=2026.10.2", "2026.10.2");
    let piano = roots.packages.join("fugue.instruments.piano");
    let mut lock = Lockfile::new();
    lock.upsert(
        "fugue.instruments.piano",
        pkg::LockedPackage {
            version: "2026.10.2".into(),
            kind: "development".into(),
            source: pkg::LockSource::Registry {
                id: "fugue.instruments.piano".into(),
                version: "2026.10.2".into(),
            },
            integrity: pkg::compute_integrity(&piano).unwrap(),
            path: piano.clone(),
            dependencies: Vec::new(),
        },
    );
    assert!(lock.verify(&roots.packages).is_ok());

    write(&piano.join("voice.json"), &json!({"modules":[]}));
    let error = lock.verify(&roots.packages).unwrap_err();
    assert!(
        error.problems[0].contains("integrity mismatch"),
        "{error:?}"
    );

    let mut stale = lock.clone();
    stale
        .packages
        .get_mut("fugue.instruments.piano")
        .unwrap()
        .version = "2026.10.1".into();
    let error = stale.verify(&roots.packages).unwrap_err();
    assert!(
        error.problems[0].contains("is not installed")
            && error.problems[0].contains("the source checkout there is version 2026.10.2"),
        "{error:?}"
    );
}

#[test]
fn invalid_source_checkout_is_a_catalog_diagnostic() {
    let (_temp, roots) = checkout("=2026.10.2", "2026.10.2");
    fs::write(
        roots
            .packages
            .join("fugue.instruments.piano/fugue.pkg.json"),
        b"{",
    )
    .unwrap();
    let page = ContentCatalog::new(roots, "session")
        .list(&ContentListQuery::default())
        .unwrap();
    assert!(
        page.diagnostics
            .iter()
            .any(|d| d.message.contains("invalid source checkout")),
        "{:?}",
        page.diagnostics
    );
}

#[test]
fn checkout_fingerprints_name_the_bytes_parsed() {
    // A checkout changes in place, and its package hash is taken after its
    // documents are read; the documents and manifest as parsed are therefore
    // fingerprinted too, so a load that parsed one revision never matches an
    // inspection of another.
    let (_temp, roots) = checkout("=2026.10.2", "2026.10.2");
    let starter_dir = roots.packages.join("fugue.starter.study");
    let piano = roots.packages.join("fugue.instruments.piano/voice.json");
    let fingerprints = roots
        .inspect(&starter(), Hashing::Full)
        .unwrap()
        .fingerprints;
    let parsed = |path: &Path| hash(&fs::read(path).unwrap());
    assert_eq!(
        fingerprints["checkout:fugue.starter.study:invention.json"],
        parsed(&starter_dir.join("invention.json"))
    );
    assert_eq!(
        fingerprints["checkout:fugue.instruments.piano:voice.json"],
        parsed(&piano)
    );
    assert!(fingerprints.contains_key("checkout-manifest:fugue.starter.study"));
    assert!(fingerprints.contains_key("checkout-manifest:fugue.instruments.piano"));

    let mut edited = voice();
    edited["title"] = json!("Edited piano");
    write(&piano, &edited);
    let after = roots
        .inspect(&starter(), Hashing::Full)
        .unwrap()
        .fingerprints;
    assert_ne!(
        after["checkout:fugue.instruments.piano:voice.json"],
        fingerprints["checkout:fugue.instruments.piano:voice.json"]
    );
}

#[test]
fn installed_fingerprints_gain_no_checkout_keys() {
    // The same packages in the installed layout keep the fingerprints (and so
    // the identity) they had before source checkouts existed.
    let (temp, roots) = checkout("=2026.10.2", "2026.10.2");
    for (id, version) in [
        ("fugue.instruments.piano", "2026.10.2"),
        ("fugue.starter.study", "2026.10.6"),
    ] {
        let staged = temp.path().join(id);
        fs::rename(roots.packages.join(id), &staged).unwrap();
        fs::create_dir_all(roots.packages.join(id)).unwrap();
        fs::rename(&staged, roots.packages.join(id).join(version)).unwrap();
    }
    let fingerprints = roots
        .inspect(&starter(), Hashing::Full)
        .unwrap()
        .fingerprints;
    assert!(fingerprints.contains_key("package:fugue.starter.study@2026.10.6"));
    assert!(
        !fingerprints.keys().any(|k| k.starts_with("checkout")),
        "{fingerprints:?}"
    );
}

#[test]
fn a_receipt_left_from_an_install_does_not_judge_a_checkout() {
    let (_temp, roots) = checkout("=2026.10.2", "2026.10.2");
    let receipt = ContentReceipt {
        package: pkg::LockedPackage {
            version: "2026.10.2".into(),
            kind: "development".into(),
            source: pkg::LockSource::Registry {
                id: "fugue.instruments.piano".into(),
                version: "2026.10.2".into(),
            },
            integrity: "sha256:from-an-earlier-install".into(),
            path: roots.packages.join("fugue.instruments.piano/2026.10.2"),
            dependencies: Vec::new(),
        },
        bundled: false,
    };
    let path = receipt_path(&roots.packages, "fugue.instruments.piano", "2026.10.2");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    roots.inspect(&starter(), Hashing::Full).unwrap();
}
