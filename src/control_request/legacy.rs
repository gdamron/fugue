//! The fallback for module types not yet on declared controls.

use super::table::TableValue;

/// Module types whose controls still take the legacy path: a control
/// surface whose setters write shared state the module reads, instead of a
/// declared table, requests and `apply`. Each family's migration removes
/// its entry; the list must end empty (the catalog-wide guardrail then
/// enforces it).
pub(crate) const LEGACY_CONTROLS: &[&str] = &[
    "adsr",
    "cell_sequencer",
    "filter",
    "lfo",
    "melody",
    "mixer",
    "sample_instrument",
    "sample_kit",
    "sample_player",
    "sample_slicer",
];

/// Why a scheduled write to a legacy control did not land. `Copy`, so a
/// refusal on the audio thread allocates nothing, where the control
/// thread's setter would format an error (see
/// [`crate::ControlSurface::set_scheduled`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LegacyRefusal {
    /// No such control, or one a schedule cannot write: read-only, a
    /// string, or a value of the other kind.
    Unsupported,
    /// A value the control does not take (a note past MIDI's range, a
    /// position past the active count).
    Invalid,
    /// A control thread holds the lock the write needs: refused rather
    /// than waited on.
    Busy,
    /// No room (a full note queue).
    Full,
}

/// `value` as a number, or refused: a boolean.
pub(crate) fn scheduled_number(value: TableValue) -> Result<f32, LegacyRefusal> {
    match value {
        TableValue::Number(number) => Ok(number),
        TableValue::Bool(_) => Err(LegacyRefusal::Unsupported),
    }
}

/// `value` as a boolean, or refused: a number.
pub(crate) fn scheduled_bool(value: TableValue) -> Result<bool, LegacyRefusal> {
    match value {
        TableValue::Bool(flag) => Ok(flag),
        TableValue::Number(_) => Err(LegacyRefusal::Unsupported),
    }
}

/// The index of a key such as `level.3` under `prefix` (`"level."`),
/// parsed without allocating.
pub(crate) fn scheduled_index(key: &str, prefix: &str) -> Option<usize> {
    key.strip_prefix(prefix)?.parse().ok()
}

#[cfg(test)]
pub(crate) mod tests;
