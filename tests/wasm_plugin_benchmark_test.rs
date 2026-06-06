#![cfg(all(feature = "plugins", not(target_arch = "wasm32")))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use fugue::{Module, Oscillator, OscillatorType};

const SAMPLE_RATE: u32 = 48_000;
// Component calls have fixed overhead, so the acceptance benchmark uses a
// low-latency block that still amortizes the WIT boundary.
const BLOCK: usize = 128;
const FRAMES: usize = SAMPLE_RATE as usize;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_core_wasm(root: &Path) -> PathBuf {
    let fixture = root.join("tests/fixtures/fugue-module-guest/Cargo.toml");
    let target_dir = root.join("target/fugue-plugin-fixtures");
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--manifest-path",
            fixture.to_str().expect("utf-8 fixture path"),
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ])
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .expect("run cargo build for fixture");
    assert!(status.success(), "fixture guest build failed");
    target_dir.join("wasm32-unknown-unknown/release/fugue_module_guest.wasm")
}

fn fixture_component(root: &Path) -> Vec<u8> {
    let core_wasm = fs::read(fixture_core_wasm(root)).expect("read fixture core wasm");
    wit_component::ComponentEncoder::default()
        .module(&core_wasm)
        .expect("read component metadata")
        .validate(true)
        .encode()
        .expect("encode component")
}

fn fixture_manifest() -> fugue::PackageManifest {
    let manifest_json = r#"{
      "id": "fugue.test.fixture",
      "version": "1.0.0",
      "kind": "module",
      "license": "MIT",
      "authors": [{ "name": "Fugue Test" }],
      "targets": ["in-graph-agent"],
      "entry": { "wasm": "fixture.fugue-module.wasm" }
    }"#;
    fugue::parse_pkg_str(manifest_json).expect("manifest")
}

fn render_native() -> Duration {
    let mut oscillator =
        Oscillator::new(SAMPLE_RATE, OscillatorType::Sawtooth).with_frequency(440.0);

    let start = Instant::now();
    for _ in 0..(FRAMES / BLOCK) {
        oscillator.process(BLOCK);
        std::hint::black_box(oscillator.output_block(0)[0]);
    }
    start.elapsed()
}

fn render_wasm(component: &[u8], manifest: &fugue::PackageManifest) -> Duration {
    let temp = tempfile::tempdir().expect("tempdir");
    let wasm_path = temp.path().join("fixture.fugue-module.wasm");
    fs::write(&wasm_path, component).expect("write component");
    let mut graph_module =
        fugue::load_component_module(&wasm_path, SAMPLE_RATE, "{}", manifest).expect("load");
    let module = graph_module.module_mut();
    module
        .set_control("frequency", 440.0)
        .expect("set frequency");

    let start = Instant::now();
    for _ in 0..(FRAMES / BLOCK) {
        module.process(BLOCK);
        std::hint::black_box(module.output_block(0)[0]);
    }
    start.elapsed()
}

#[test]
#[ignore = "local performance check; run with --ignored --nocapture"]
fn wasm_fixture_oscillator_overhead_is_within_target() {
    let root = repo_root();
    let component = fixture_component(&root);
    let manifest = fixture_manifest();

    // Warm cache and JIT paths before measuring.
    std::hint::black_box(render_native());
    std::hint::black_box(render_wasm(&component, &manifest));

    let native = render_native();
    let wasm = render_wasm(&component, &manifest);
    let ratio = wasm.as_secs_f64() / native.as_secs_f64();

    eprintln!("wasm fixture oscillator: native={native:?}, wasm={wasm:?}, overhead={ratio:.2}x");
    assert!(
        ratio <= 2.0,
        "WASM oscillator overhead target is <=2x; measured {ratio:.2}x"
    );
}
