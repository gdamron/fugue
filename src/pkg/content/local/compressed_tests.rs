//! Compressed (gzip) documents are listed, loaded, capped and
//! verified like plain ones.

use super::*;
use crate::pkg::read_limit::DOCUMENT_READ_LIMIT;
use serde_json::json;
use std::io::Write;

fn roots() -> (tempfile::TempDir, ContentRoots) {
    let temp = tempfile::tempdir().unwrap();
    let roots = ContentRoots {
        packages: temp.path().join("packs"),
        workspace: temp.path().join("inventions"),
    };
    (temp, roots)
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// A one-oscillator development; without `outputs`, an invention.
fn piece(frequency: f64) -> Vec<u8> {
    document(frequency, true)
}

fn document(frequency: f64, development: bool) -> Vec<u8> {
    let mut value = json!({
        "modules": [{ "id": "osc", "type": "oscillator", "config": { "frequency": frequency } }],
        "connections": []
    });
    if development {
        value["outputs"] = json!([{ "name": "audio", "from": "osc", "from_port": "audio" }]);
    }
    serde_json::to_vec(&value).unwrap()
}

fn query(reference: ContentRef) -> ContentDetailQuery {
    ContentDetailQuery {
        schema_version: 1,
        reference,
    }
}

#[test]
fn compressed_workspace_documents_list_and_load_beside_plain_ones() {
    let (_temp, roots) = roots();
    write(
        &roots.workspace.join("packed.json"),
        &gzip(&document(330.0, false)),
    );
    write(&roots.workspace.join("plain.json"), &document(440.0, false));
    let mut catalog = ContentCatalog::new(roots, "session");
    let page = catalog.list(&ContentListQuery::default()).unwrap();
    assert_eq!(page.items.len(), 2, "{:?}", page.diagnostics);
    for item in page.items {
        let loaded = catalog.load_invention(&query(item.reference)).unwrap();
        assert_eq!(loaded.modules[0].module_type, "oscillator");
    }
}

#[test]
fn a_document_that_expands_past_the_limit_is_too_large_naming_the_file() {
    let (_temp, roots) = roots();
    write(
        &roots.workspace.join("bomb.json"),
        &gzip(&vec![b' '; DOCUMENT_READ_LIMIT as usize + 1]),
    );
    let page = ContentCatalog::new(roots, "session")
        .list(&ContentListQuery::default())
        .unwrap();
    assert_eq!(page.diagnostics.len(), 1);
    let diagnostic = &page.diagnostics[0];
    assert_eq!(diagnostic.code, "file_too_large");
    assert!(diagnostic.message.contains("bomb.json"), "{diagnostic}");
    assert!(diagnostic.message.contains("16 MiB"), "{diagnostic}");
}

#[test]
fn a_locked_compressed_document_is_verified_by_its_bytes_on_disk() {
    let (_temp, roots) = roots();
    let dir = roots.packages.join("fugue.test.voice/1.0.0");
    write(
        &dir.join("fugue.pkg.json"),
        &serde_json::to_vec(&json!({"id":"fugue.test.voice","version":"1.0.0","kind":"development","license":"MIT","authors":[{"name":"Test"}],"targets":["external-agent"],"entry":{"development":"voice.json"}})).unwrap(),
    );
    write(&dir.join("voice.json"), &gzip(&piece(220.0)));
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
    write(
        &roots.packages.parent().unwrap().join(pkg::LOCKFILE_NAME),
        &lock.to_bytes().unwrap(),
    );
    let reference = ContentRef::Package {
        package: "fugue.test.voice".into(),
        version: "1.0.0".into(),
    };
    let mut catalog = ContentCatalog::new(roots, "session");
    catalog.detail(&query(reference.clone())).unwrap();
    // The same document compressed harder: the same JSON, other bytes.
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::none());
    encoder.write_all(&piece(220.0)).unwrap();
    write(&dir.join("voice.json"), &encoder.finish().unwrap());
    assert_eq!(
        catalog.detail(&query(reference)).unwrap_err().code,
        "integrity_mismatch"
    );
}
