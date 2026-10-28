//! The host side of the input and output paths makes no allocation per call.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::alloc_counter::allocator_events;

/// Points the compiled-component cache at a fresh directory until dropped,
/// so a test never writes under the real `~/.fugue`. The environment is
/// process-wide, so the tests that load a component take turns.
pub(crate) struct TempCache {
    previous: Option<OsString>,
    _dir: tempfile::TempDir,
    _turn: MutexGuard<'static, ()>,
}

pub(crate) fn temp_cache() -> TempCache {
    static TURN: OnceLock<Mutex<()>> = OnceLock::new();
    let turn = TURN
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempfile::tempdir().expect("cache tempdir");
    let previous = std::env::var_os(super::CACHE_DIR_ENV);
    std::env::set_var(super::CACHE_DIR_ENV, dir.path());
    TempCache {
        previous,
        _dir: dir,
        _turn: turn,
    }
}

impl Drop for TempCache {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var(super::CACHE_DIR_ENV, value),
            None => std::env::remove_var(super::CACHE_DIR_ENV),
        }
    }
}

/// Builds the fixture guest component (the one `tests/plugin_wit_test.rs`
/// loads) and returns its path, kept alive by the returned tempdir.
pub(crate) fn fixture_component() -> (tempfile::TempDir, PathBuf) {
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

pub(crate) fn fixture_manifest() -> crate::pkg::PackageManifest {
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
    let _cache = temp_cache();
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

#[test]
fn set_input_block_lowers_the_input_block_without_a_host_allocation() {
    let _cache = temp_cache();
    let (_temp, path) = fixture_component();
    let mut module =
        super::WasmModule::load(&path, 48_000, "{}", &fixture_manifest()).expect("load");
    module.init_ports().expect("ports");
    module.input_blocks[0][..64].fill(24_000.0);
    // Warm up the export.
    let call = |module: &mut super::WasmModule| {
        module
            .set_input_block
            .call(&mut module.store, (0, &module.input_blocks[0][..64]))
            .map(|(result,)| result)
    };
    call(&mut module).expect("call").expect("set input block");

    let (result, allocs, frees) = allocator_events(|| call(&mut module));

    assert!(matches!(result, Ok(Ok(()))));
    assert_eq!((allocs, frees), (0, 0));
}

#[test]
fn process_copies_the_emitted_output_without_a_host_allocation() {
    let _cache = temp_cache();
    let (_temp, path) = fixture_component();
    let mut graph_module =
        super::load_component_module(&path, 48_000, "{}", &fixture_manifest()).expect("load");
    let module = graph_module.module_mut();
    module.set_input_connected(0, true);
    module.input_block_mut(0)[..64].fill(12_000.0);
    // Warm up: the first block may set up guest state.
    assert!(module.process(64));

    let (active, allocs, frees) = allocator_events(|| module.process(64));

    assert!(active);
    assert_eq!((allocs, frees), (0, 0));
    // The fixture's phase ramps by a quarter per sample, from where the
    // warm-up block left it.
    assert_eq!(&module.output_block(0)[..4], &[0.0, 0.25, 0.5, 0.75]);
}
