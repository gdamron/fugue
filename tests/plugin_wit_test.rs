#[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
#[test]
fn default_registry_exposes_wasm_module_factory() {
    let registry = fugue::ModuleRegistry::default();
    assert!(registry.has_type("wasm_module"));
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
