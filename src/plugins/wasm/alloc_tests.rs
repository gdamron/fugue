//! The host side of the input path makes no allocation per call.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::alloc_counter::allocator_events;

/// Builds the fixture guest component (the one `tests/plugin_wit_test.rs`
/// loads) and returns its path, kept alive by the returned tempdir.
fn fixture_component() -> (tempfile::TempDir, PathBuf) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dir = root.join("target/fugue-plugin-fixtures");
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--manifest-path",
            "tests/fixtures/fugue-module-guest/Cargo.toml",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ])
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .expect("run cargo build for fixture");
    assert!(status.success(), "fixture guest build failed");
    let core = fs::read(target_dir.join("wasm32-unknown-unknown/release/fugue_module_guest.wasm"))
        .expect("read fixture core wasm");
    let component = wit_component::ComponentEncoder::default()
        .module(&core)
        .expect("read component metadata")
        .validate(true)
        .encode()
        .expect("encode component");
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("fixture.fugue-module.wasm");
    fs::write(&path, component).expect("write component");
    (temp, path)
}

fn fixture_manifest() -> crate::pkg::PackageManifest {
    crate::pkg::parse_str(
        r#"{
          "id": "fugue.test.fixture",
          "version": "1.0.0",
          "kind": "module",
          "license": "MIT",
          "authors": [{ "name": "Fugue Test" }],
          "targets": ["in-graph-agent"],
          "entry": { "wasm": "fixture.fugue-module.wasm" }
        }"#,
    )
    .expect("manifest")
}

#[test]
fn set_input_and_set_control_make_no_host_allocation() {
    let (_temp, path) = fixture_component();
    let mut graph_module =
        super::load_component_module(&path, 48_000, "{}", &fixture_manifest()).expect("load");
    let module = graph_module.module_mut();
    // Warm up: the first call into each export may set up guest state.
    module.set_input("frequency", 440.0).expect("set input");
    module.set_control("frequency", 440.0).expect("set control");

    let (results, allocs, frees) = allocator_events(|| {
        (
            module.set_input("frequency", 220.0),
            module.set_control("frequency", 220.0),
        )
    });

    assert_eq!(results, (Ok(()), Ok(())));
    assert_eq!((allocs, frees), (0, 0));
}
