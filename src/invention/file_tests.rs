use super::*;
use crate::invention::Invention;
use crate::pkg::read_limit::test_override::with_document_limit;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs;
use std::io::Write;

const DOCUMENT: &str = r#"{
  "version": "1.0.0",
  "title": "Two voices",
  "modules": [
    { "id": "voice", "type": "oscillator", "config": { "waveform": "sine", "frequency": 220 } },
    { "id": "out", "type": "dac" }
  ],
  "connections": [{ "from": "voice", "to": "out" }]
}
"#;

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn written(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    fs::write(&path, bytes).unwrap();
    (dir, path)
}

#[test]
fn a_compressed_file_loads_as_its_json() {
    let (_dir, path) = written("piece.fyug", &gzip(DOCUMENT.as_bytes()));
    let loaded = Invention::from_file(path.to_str().unwrap()).unwrap();
    let plain = Invention::from_json(DOCUMENT).unwrap();
    assert_eq!(loaded.modules.len(), 2);
    assert_eq!(loaded.connections.len(), 1);
    assert_eq!(loaded.title, plain.title);
    assert_eq!(loaded.source_path.as_deref(), Some(path.as_path()));
}

#[test]
fn files_are_told_apart_by_content_not_name() {
    let (_a, plain_named_fyug) = written("plain.fyug", DOCUMENT.as_bytes());
    let (_b, gzip_named_json) = written("packed.json", &gzip(DOCUMENT.as_bytes()));
    for path in [plain_named_fyug, gzip_named_json] {
        let file = read(&path).unwrap();
        assert_eq!(file.text, DOCUMENT, "{}", path.display());
    }
}

#[test]
fn the_bytes_on_disk_are_kept_for_hashing() {
    let packed = gzip(DOCUMENT.as_bytes());
    let (_dir, path) = written("piece.fyug", &packed);
    assert_eq!(read(&path).unwrap().bytes, packed);
}

#[test]
fn concatenated_members_read_as_gunzip_reads_them() {
    let (head, tail) = DOCUMENT.split_at(40);
    let mut packed = gzip(head.as_bytes());
    packed.extend(gzip(tail.as_bytes()));
    let (_dir, path) = written("piece.fyug", &packed);
    assert_eq!(read(&path).unwrap().text, DOCUMENT);
}

#[test]
fn a_file_that_expands_past_the_limit_is_refused() {
    let packed = gzip(&vec![b' '; 4096]);
    let (_dir, path) = written("bomb.fyug", &packed);
    let error = with_document_limit(4095, || read(&path)).err().unwrap();
    assert!(matches!(error, DocumentFileError::ExpandsTooFar { .. }));
    let message = error.to_string();
    assert!(message.contains("bomb.fyug"), "{message}");
    assert!(message.contains("expands past"), "{message}");
    // At the limit exactly, it reads.
    assert!(with_document_limit(4096, || read(&path)).is_ok());
}

#[test]
fn the_real_limit_is_sixteen_mebibytes() {
    let packed = gzip(&vec![b' '; 16 * 1024 * 1024 + 1]);
    let (_dir, path) = written("bomb.fyug", &packed);
    let error = Invention::from_file(path.to_str().unwrap()).err().unwrap();
    assert!(error.to_string().contains("16 MiB"), "{error}");
}

#[test]
fn corrupt_and_truncated_files_are_refused_naming_the_file() {
    let packed = gzip(DOCUMENT.as_bytes());
    let mut corrupt = packed.clone();
    let middle = corrupt.len() / 2;
    corrupt[middle] ^= 0xff;
    let truncated = packed[..packed.len() - 6].to_vec();
    let mut trailing = packed;
    trailing.extend_from_slice(b"not gzip");
    for (name, bytes) in [
        ("corrupt.fyug", corrupt),
        ("truncated.fyug", truncated),
        ("trailing.fyug", trailing),
        ("magic-only.fyug", vec![0x1f, 0x8b]),
    ] {
        let (_dir, path) = written(name, &bytes);
        let error = Invention::from_file(path.to_str().unwrap()).err().unwrap();
        let message = error.to_string();
        assert!(message.contains(name), "{message}");
        assert!(message.contains("cannot be decompressed"), "{message}");
    }
}

#[test]
fn a_plain_file_is_held_to_the_limit_too() {
    let (_dir, path) = written("piece.json", DOCUMENT.as_bytes());
    let size = DOCUMENT.len() as u64;
    let error = with_document_limit(size - 1, || Invention::from_file(path.to_str().unwrap()))
        .err()
        .unwrap();
    assert!(error.to_string().contains("piece.json"), "{error}");
    assert!(with_document_limit(size, || Invention::from_file(path.to_str().unwrap())).is_ok());
}

#[test]
fn a_document_that_is_not_text_is_refused() {
    let (_dir, path) = written("piece.fyug", &gzip(&[b'{', 0xff, b'}']));
    let error = read(&path).err().unwrap();
    assert!(matches!(error, DocumentFileError::NotText { .. }));
    assert!(error.to_string().contains("piece.fyug"));
}
