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
    "code",
    "control_scheduler",
    "divisi",
    "filter",
    "lfo",
    "melody",
    "mixer",
    "oscillator",
    "reverb",
    "sample_instrument",
    "sample_kit",
    "sample_player",
    "sample_slicer",
    "step_sequencer",
    "sustain",
];

#[cfg(test)]
mod tests {
    use super::LEGACY_CONTROLS;

    /// Every built-in type with controls either declares them or is listed,
    /// never both.
    #[test]
    fn a_module_type_declares_its_controls_or_is_listed_legacy() {
        let registry = crate::ModuleRegistry::default();
        let mut checked = Vec::new();
        for type_id in registry.types() {
            if registry.is_sink(type_id) {
                continue;
            }
            let Ok(result) = registry.build(type_id, 48_000, &serde_json::json!({})) else {
                continue;
            };
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
        let types: Vec<_> = registry.types().collect();
        for listed in LEGACY_CONTROLS {
            assert!(types.contains(listed), "'{listed}' is not a module type");
        }
        for expected in ["oscillator", "vca", "mixer"] {
            assert!(
                checked.contains(&expected),
                "'{expected}' not checked: {checked:?}"
            );
        }
    }
}
