//! The fallback for module types not yet on declared controls.

/// Module types whose controls still take the legacy path: a control
/// surface whose setters write shared state the module reads, instead of a
/// declared table, requests and `apply`. Each family's migration removes
/// its entry; the list must end empty (the catalog-wide guardrail then
/// enforces it).
pub(crate) const LEGACY_CONTROLS: &[&str] = &[
    "adsr",
    "agent",
    "cell_sequencer",
    "clock",
    "control_scheduler",
    "filter",
    "lfo",
    "melody",
    "mixer",
    "sample_instrument",
    "sample_kit",
    "sample_player",
    "sample_slicer",
];

#[cfg(test)]
mod tests {
    use super::LEGACY_CONTROLS;
    use crate::module_config::tests::registry::{base_config, UNBUILDABLE};

    /// Every built-in type with controls either declares them or is listed,
    /// never both; every type builds (from the registry harness's config)
    /// unless it cannot be built in a test at all, and every listed type
    /// is checked.
    #[test]
    fn a_module_type_declares_its_controls_or_is_listed_legacy() {
        let registry = crate::ModuleRegistry::default();
        let mut checked = Vec::new();
        for type_id in registry.types() {
            if registry.is_sink(type_id) || UNBUILDABLE.iter().any(|(id, _)| *id == type_id) {
                continue;
            }
            let mut config = base_config(type_id);
            if type_id == "sample_slicer" {
                // Its controls exist only in elastic mode.
                config["mode"] = serde_json::json!("elastic");
            }
            let built = registry.build(type_id, 48_000, &config);
            let result = built.unwrap_or_else(|error| panic!("'{type_id}' did not build: {error}"));
            if result.control_surface.is_none() {
                continue;
            }
            let declared = result.module.module().declared().is_some();
            let listed = LEGACY_CONTROLS.contains(&type_id);
            assert!(
                declared != listed,
                "'{type_id}': declared {declared}, listed {listed}"
            );
            checked.push(type_id);
        }
        for listed in LEGACY_CONTROLS {
            assert!(
                checked.contains(listed),
                "'{listed}' was not checked: {checked:?}"
            );
        }
        for expected in ["oscillator", "vca"] {
            assert!(
                checked.contains(&expected),
                "'{expected}' not checked: {checked:?}"
            );
        }
    }
}
