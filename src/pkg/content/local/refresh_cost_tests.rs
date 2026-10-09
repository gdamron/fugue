//! FUG-279: catalog refresh cost is bounded by file count, not library bytes.

use super::*;
use serde_json::json;
use std::time::{Duration, Instant, SystemTime};

fn write(path: &Path, value: &serde_json::Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

/// A sparse file, backdated so it counts as settled (see the cache's settle window).
fn sparse(path: &Path, size: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = fs::File::create(path).unwrap();
    file.set_len(size).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
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
