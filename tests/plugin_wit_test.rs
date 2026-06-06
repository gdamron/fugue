#[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
#[test]
fn default_registry_exposes_wasm_module_factory() {
    let registry = fugue::ModuleRegistry::default();
    assert!(registry.has_type("wasm_module"));
}

#[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
#[test]
fn loads_fixture_component_and_processes_audio_block() {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn build_guest_core_wasm(root: &Path) -> PathBuf {
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

    let root = repo_root();
    let core_wasm = fs::read(build_guest_core_wasm(&root)).expect("read fixture core wasm");
    let component = wit_component::ComponentEncoder::default()
        .module(&core_wasm)
        .expect("read component metadata")
        .validate(true)
        .encode()
        .expect("encode component");

    let temp = tempfile::tempdir().expect("tempdir");
    let wasm_path = temp.path().join("fixture.fugue-module.wasm");
    fs::write(&wasm_path, component).expect("write component");

    let manifest_json = r#"{
      "id": "fugue.test.fixture",
      "version": "1.0.0",
      "kind": "module",
      "license": "MIT",
      "authors": [{ "name": "Fugue Test" }],
      "targets": ["in-graph-agent"],
      "entry": { "wasm": "fixture.fugue-module.wasm" }
    }"#;
    let manifest = fugue::parse_pkg_str(manifest_json).expect("manifest");
    let mut graph_module =
        fugue::load_component_module(&wasm_path, 48_000, "{}", &manifest).expect("load component");
    let module = graph_module.module_mut();

    assert_eq!(module.name(), "FixtureOscillator");
    assert_eq!(module.inputs(), &["frequency"]);
    assert_eq!(module.outputs(), &["audio"]);

    module.input_block_mut(0)[..2].fill(48_000.0);
    assert!(module.process(2));
    assert_eq!(module.output_block(0)[0], 0.0);
    assert_eq!(module.output_block(0)[1], 0.0);

    module.input_block_mut(0)[..2].fill(24_000.0);
    assert!(module.process(2));
    assert_eq!(module.get_output("audio").expect("get output"), 0.0);
    assert_eq!(module.output_block(0)[1], 0.5);

    module.set_input("frequency", 12_000.0).expect("set frequency");
    module.set_control("frequency", 12_000.0).expect("set control");
}

#[test]
fn fugue_module_wit_declares_required_exports() {
    let wit = include_str!("../wit/fugue-module.wit");
    for export in [
        "export init:",
        "export set-input:",
        "export process:",
        "export get-output:",
        "export set-control:",
    ] {
        assert!(wit.contains(export), "missing WIT export {export}");
    }
}
