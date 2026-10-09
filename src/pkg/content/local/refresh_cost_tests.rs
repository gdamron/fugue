//! FUG-279: catalog refresh cost is bounded by file count, not library bytes.

use super::cache::test_counter::hashes_during;
use super::*;
use serde_json::json;
use std::time::{Duration, Instant, SystemTime};

/// Backdate a file so it counts as settled, as an installed library would be.
fn settle(path: &Path) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
}

fn write(path: &Path, value: &serde_json::Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    settle(path);
}

/// A sparse file, so a large library costs no disk.
fn sparse(path: &Path, size: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::File::create(path).unwrap().set_len(size).unwrap();
    settle(path);
}

fn sampler(asset: &str) -> serde_json::Value {
    json!({
        "modules":[{"id":"voice","type":"sample_player","config":{"asset":asset}}],
        "connections":[],
        "outputs":[{"name":"audio","from":"voice","from_port":"audio"}]
    })
}

/// A sample library of `files` × `file_size` sparse bytes, a development
/// package that plays it through `pkg:`, a workspace development that plays
/// it too, and a workspace invention with its own long local sample.
fn sample_heavy_library(roots: &ContentRoots, files: usize, file_size: u64) -> ContentRef {
    let library = roots.packages.join("fugue.test.library/1.0.0");
    write(
        &library.join("fugue.pkg.json"),
        &json!({"id":"fugue.test.library","version":"1.0.0","kind":"sample-pack","license":"CC0-1.0","authors":[{"name":"Test"}],"targets":["external-agent"],"entry":{"samples":"samples.json"}}),
    );
    for index in 0..files {
        sparse(&library.join(format!("s{index}.wav")), file_size);
    }
    let sampler_root = roots.packages.join("fugue.test.sampler/1.0.0");
    write(
        &sampler_root.join("fugue.pkg.json"),
        &json!({"id":"fugue.test.sampler","version":"1.0.0","kind":"development","license":"MIT","authors":[{"name":"Test"}],"targets":["external-agent"],"deps":["fugue.test.library@^1.0.0"],"entry":{"development":"voice.json"}}),
    );
    write(
        &sampler_root.join("voice.json"),
        &sampler("fugue.test.library@1.0.0:s0.wav"),
    );
    write(
        &roots.workspace.join("pad.json"),
        &sampler("fugue.test.library@1.0.0:s1.wav"),
    );
    sparse(&roots.workspace.join("kick.wav"), file_size);
    let mut kit = sampler("kick.wav");
    kit.as_object_mut().unwrap().remove("outputs");
    write(&roots.workspace.join("kit.json"), &kit);
    ContentRef::Package {
        package: "fugue.test.sampler".into(),
        version: "1.0.0".into(),
    }
}

fn roots() -> (tempfile::TempDir, ContentRoots) {
    let temp = tempfile::tempdir().unwrap();
    let roots = ContentRoots {
        packages: temp.path().join("packs"),
        workspace: temp.path().join("inventions"),
    };
    (temp, roots)
}

fn list(catalog: &mut ContentCatalog) -> ContentPage {
    catalog.list(&ContentListQuery::default()).unwrap()
}

#[test]
fn warm_list_and_detail_reuse_digests_until_a_file_changes() {
    let (_temp, roots) = roots();
    let reference = sample_heavy_library(&roots, 2, 4096);
    let mut catalog = ContentCatalog::new(roots.clone(), "session");
    let mut cold = None;
    // The library is hashed once even though two entries depend on it.
    assert_eq!(hashes_during(|| cold = Some(list(&mut catalog))), 3);
    let cold = cold.unwrap();
    assert_eq!(cold.items.len(), 3, "{:?}", cold.diagnostics);
    assert_eq!(
        hashes_during(|| {
            assert_eq!(list(&mut catalog).generation, cold.generation);
            catalog.detail(&query(reference.clone())).unwrap();
        }),
        0
    );
    // Growing one sample changes the library's stamp, and only the library re-hashes.
    sparse(
        &roots.packages.join("fugue.test.library/1.0.0/s1.wav"),
        8192,
    );
    assert_eq!(hashes_during(|| drop(list(&mut catalog))), 1);
    // A newly installed version shows up with no invalidation step.
    let sampler_dir = roots.packages.join("fugue.test.sampler");
    copy_dir(&sampler_dir.join("1.0.0"), &sampler_dir.join("1.1.0"));
    let manifest = sampler_dir.join("1.1.0/fugue.pkg.json");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        text.replace("\"version\":\"1.0.0\"", "\"version\":\"1.1.0\""),
    )
    .unwrap();
    let page = list(&mut catalog);
    assert_eq!(page.items.len(), 4, "{:?}", page.diagnostics);
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

#[test]
fn import_rehashes_and_rejects_a_change_the_metadata_stamp_cannot_see() {
    let (_temp, roots) = roots();
    let kick = roots.workspace.join("kick.wav");
    sparse(&kick, 4096);
    write(&roots.workspace.join("drum.json"), &sampler("kick.wav"));
    let mut catalog = ContentCatalog::new(roots.clone(), "session");
    let reference = list(&mut catalog).items[0].reference.clone();
    assert_eq!(hashes_during(|| drop(list(&mut catalog))), 0);
    let warm_import = hashes_during(|| drop(roots.load_development(&reference).unwrap()));
    assert!(warm_import > 0, "imports never trust the cache");
    // Same size, mtime restored: (size, mtime) alone cannot tell.
    let modified = fs::metadata(&kick).unwrap().modified().unwrap();
    fs::write(&kick, vec![1u8; 4096]).unwrap();
    fs::File::options()
        .write(true)
        .open(&kick)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let error = roots.load_development(&reference).unwrap_err();
    assert_eq!(error.code, "stale_reference");
    // Where the stamp includes ctime, listing notices the rewrite too.
    #[cfg(unix)]
    assert_ne!(list(&mut catalog).items[0].reference, reference);
}

#[test]
fn concurrent_catalogs_share_the_cache_safely() {
    let (_temp, roots) = roots();
    sample_heavy_library(&roots, 2, 4096);
    let expected = list(&mut ContentCatalog::new(roots.clone(), "seed")).items;
    let threads: Vec<_> = (0..4)
        .map(|i| {
            let roots = roots.clone();
            std::thread::spawn(move || {
                let mut catalog = ContentCatalog::new(roots, format!("session-{i}"));
                (0..5).map(|_| list(&mut catalog).items).collect::<Vec<_>>()
            })
        })
        .collect();
    for thread in threads {
        for items in thread.join().unwrap() {
            assert_eq!(items, expected);
        }
    }
}

fn timed<T>(body: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let result = body();
    (result, start.elapsed())
}

/// Before/after measurement for FUG-279. Run with
/// `cargo test --lib refresh_cost_benchmark -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark: hashes several GiB of sparse files"]
fn refresh_cost_benchmark() {
    const MIB: u64 = 1024 * 1024;
    for (files, file_size) in [(4, 64 * MIB), (16, 256 * MIB)] {
        let temp = tempfile::tempdir().unwrap();
        let roots = ContentRoots {
            packages: temp.path().join("packs"),
            workspace: temp.path().join("inventions"),
        };
        let reference = sample_heavy_library(&roots, files, file_size);
        let mut catalog = ContentCatalog::new(roots.clone(), "session");
        let (page, cold) = timed(|| catalog.list(&ContentListQuery::default()).unwrap());
        assert_eq!(page.items.len(), 3, "{:?}", page.diagnostics);
        let warm: Vec<_> = (0..5)
            .map(|_| timed(|| catalog.list(&ContentListQuery::default()).unwrap()).1)
            .collect();
        let (_, detail) = timed(|| catalog.detail(&query(reference.clone())).unwrap());
        let (_, import) = timed(|| roots.load_development(&reference).unwrap());
        let library = files as u64 * file_size / MIB;
        println!(
            "library {library} MiB ({files} files): list cold {cold:.2?}, list warm {:.2?} (mean of 5), detail {detail:.2?}, load_development {import:.2?}",
            warm.iter().sum::<Duration>() / 5
        );
    }
}

fn query(reference: ContentRef) -> ContentDetailQuery {
    ContentDetailQuery {
        schema_version: 1,
        reference,
    }
}
