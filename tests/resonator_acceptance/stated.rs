//! The single place that touches the implementation's stated ring time, so
//! the rest of the suite depends on it through one function.

/// The stated time (seconds) within which the output after one strike falls
/// below -90 dBFS and stays there, for `model` at `damping`.
pub fn stated_ring(model: &str, damping: f32) -> f32 {
    fugue::modules::resonator::ring_seconds(model, damping)
        .unwrap_or_else(|| panic!("ring_seconds knows model {model} (damping {damping})"))
}

/// `ring_seconds` for a model name the module does not have.
pub fn stated_ring_for_unknown(model: &str) -> Option<f32> {
    fugue::modules::resonator::ring_seconds(model, 0.5)
}
