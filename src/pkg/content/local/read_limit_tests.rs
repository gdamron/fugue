//! FUG-280: audio assets are stream-hashed under one cap on both the
//! workspace (local path) and package (`pkg:`) paths.

use super::*;
use crate::pkg::read_limit::{test_override::with_hashed_limit, DOCUMENT_READ_LIMIT};
use serde_json::json;

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

fn uses_sample(asset: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "modules":[{"id":"kick","type":"sample_player","config":{"asset":asset}}],
        "connections":[]
    }))
    .unwrap()
}

/// The same `audio` bytes, once as a workspace file next to `local.json` and
/// once inside a sample pack that `packaged.json` references via `pkg:`.
fn same_sample_both_ways(roots: &ContentRoots, audio: &[u8]) {
    let pack = roots.packages.join("fugue.test.samples/1.0.0");
    write(
        &pack.join("fugue.pkg.json"),
        br#"{"id":"fugue.test.samples","version":"1.0.0","kind":"sample-pack","license":"CC0-1.0","authors":[{"name":"Test"}],"targets":["external-agent"],"entry":{"samples":"samples.json"}}"#,
    );
    write(&pack.join("long.wav"), audio);
    write(&roots.workspace.join("long.wav"), audio);
    write(
        &roots.workspace.join("local.json"),
        &uses_sample("long.wav"),
    );
    write(
        &roots.workspace.join("packaged.json"),
        &uses_sample("fugue.test.samples@1.0.0:long.wav"),
    );
}

#[test]
fn same_audio_bytes_are_accepted_or_refused_on_both_paths() {
    // Small enough to cross with a tiny file, large enough for the manifest,
    // which package integrity hashes under the same cap.
    const LIMIT: u64 = 4096;
    for (size, accepted) in [(LIMIT, true), (LIMIT + 1, false)] {
        with_hashed_limit(LIMIT, || {
            let (_temp, roots) = roots();
            same_sample_both_ways(&roots, &vec![7u8; size as usize]);
            let page = ContentCatalog::new(roots, "session")
                .list(&ContentListQuery::default())
                .unwrap();
            if accepted {
                assert_eq!(page.items.len(), 2, "{:?}", page.diagnostics);
                assert!(page.diagnostics.is_empty());
            } else {
                assert!(page.items.is_empty(), "{:?}", page.items);
                assert_eq!(page.diagnostics.len(), 2);
                for diagnostic in &page.diagnostics {
                    assert_eq!(diagnostic.code, "file_too_large", "{diagnostic}");
                    assert!(diagnostic.message.contains("long.wav"), "{diagnostic}");
                    assert!(diagnostic.message.contains("4096 bytes"), "{diagnostic}");
                    assert!(
                        diagnostic.message.contains("trim or split the sample"),
                        "{diagnostic}"
                    );
                }
            }
        });
    }
}

#[test]
fn workspace_audio_over_the_document_limit_lists_and_imports() {
    let (_temp, roots) = roots();
    fs::create_dir_all(&roots.workspace).unwrap();
    // Sparse, so it costs no disk; ~95 s of CD audio used to be the ceiling.
    fs::File::create(roots.workspace.join("long.wav"))
        .unwrap()
        .set_len(DOCUMENT_READ_LIMIT + 1)
        .unwrap();
    write(&roots.workspace.join("song.json"), &uses_sample("long.wav"));
    let mut catalog = ContentCatalog::new(roots, "session");
    let page = catalog.list(&ContentListQuery::default()).unwrap();
    assert_eq!(page.items.len(), 1, "{:?}", page.diagnostics);
    let reference = page.items[0].reference.clone();
    let loaded = catalog
        .load_invention(&ContentDetailQuery {
            schema_version: 1,
            reference,
        })
        .unwrap();
    assert_eq!(loaded.modules[0].module_type, "sample_player");
}

#[test]
fn oversized_documents_name_the_file_and_the_limit() {
    let (_temp, roots) = roots();
    let mut document = uses_sample("long.wav");
    // Valid JSON padded past the document limit.
    document.truncate(document.len() - 1);
    document.extend(std::iter::repeat_n(b' ', DOCUMENT_READ_LIMIT as usize));
    document.push(b'}');
    write(&roots.workspace.join("huge.json"), &document);
    let page = ContentCatalog::new(roots, "session")
        .list(&ContentListQuery::default())
        .unwrap();
    assert_eq!(page.diagnostics.len(), 1);
    let diagnostic = &page.diagnostics[0];
    assert_eq!(diagnostic.code, "file_too_large");
    assert!(diagnostic.message.contains("huge.json"), "{diagnostic}");
    assert!(
        diagnostic.message.contains("limited to 16 MiB"),
        "{diagnostic}"
    );
}
