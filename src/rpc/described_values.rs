//! Control values too large to describe. A module's description and its
//! listed controls carry each control's value (and, listed, its default,
//! which for a running instance is the same value). A value larger than
//! [`MAX_DESCRIBED_VALUE_BYTES`], such as a cell sequencer's whole bank of
//! cells, is left out of both and named in `values_omitted` with its size,
//! so a description never fails for size because of one control. A reader
//! takes the value through `get_control`, whole or, for a bank of cells,
//! one cell at a time as `cells.N`.

use crate::{ControlMeta, ControlValue, StoredCells};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The largest control value, as JSON, a description or control listing
/// carries; a larger one is named in `values_omitted` instead.
pub const MAX_DESCRIBED_VALUE_BYTES: usize = 8 * 1024;

/// A control value a description left out for its size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct OmittedValue {
    /// The value's size as JSON, in bytes.
    pub bytes: u64,
    /// For a bank of cells: each cell's length in steps, in order, so the
    /// bank can be shown and read one cell at a time without reading it
    /// whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_lengths: Option<Vec<u32>>,
}

/// Counts the bytes written to it.
struct Counter(usize);

impl std::io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The size of `value` as JSON, in bytes.
fn json_bytes(value: &impl Serialize) -> usize {
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_or(usize::MAX, |()| counter.0)
}

/// Each cell's length in steps, when `value` is a bank of cells as text.
fn cell_lengths(value: &ControlValue) -> Option<Vec<u32>> {
    use serde::de::IgnoredAny;
    let bank: Vec<Vec<IgnoredAny>> = serde_json::from_str(value.as_string().ok()?).ok()?;
    bank.iter()
        .map(|cell| u32::try_from(cell.len()).ok())
        .collect()
}

/// The controls of `controls` whose value in `values` or whose listed
/// default is larger than [`MAX_DESCRIBED_VALUE_BYTES`]; `cells`, when the
/// module stores a bank of cells, says which control holds it.
pub fn omitted_values(
    controls: &[ControlMeta],
    values: &BTreeMap<String, ControlValue>,
    cells: Option<&StoredCells>,
) -> BTreeMap<String, OmittedValue> {
    let mut omitted = BTreeMap::new();
    for meta in controls {
        let value = values.get(&meta.key);
        let bytes = json_bytes(&meta.default).max(value.map_or(0, json_bytes));
        if bytes > MAX_DESCRIBED_VALUE_BYTES {
            let is_bank = cells.is_some_and(|cells| cells.cell_key == meta.key);
            let lengths = is_bank.then(|| cell_lengths(value.unwrap_or(&meta.default)));
            omitted.insert(
                meta.key.clone(),
                OmittedValue {
                    bytes: bytes as u64,
                    cell_lengths: lengths.flatten(),
                },
            );
        }
    }
    omitted
}

/// Leaves the omitted values out of `values` and stands in an empty text
/// for each omitted control's default in `controls`; serializing drops it
/// (see [`leave_out_defaults`]).
pub(crate) fn leave_out(
    controls: &mut [ControlMeta],
    values: &mut BTreeMap<String, ControlValue>,
    omitted: &BTreeMap<String, OmittedValue>,
) {
    values.retain(|key, _| !omitted.contains_key(key));
    for meta in controls.iter_mut() {
        if omitted.contains_key(&meta.key) {
            meta.default = ControlValue::String(String::new());
        }
    }
}

/// Drops `default` from each control in `controls`, a JSON list of
/// [`ControlMeta`], whose value is omitted.
pub fn leave_out_defaults(
    controls: &mut serde_json::Value,
    omitted: &BTreeMap<String, OmittedValue>,
) {
    for control in controls.as_array_mut().into_iter().flatten() {
        let key = control.get("key").and_then(|key| key.as_str());
        if key.is_some_and(|key| omitted.contains_key(key)) {
            if let Some(control) = control.as_object_mut() {
                control.remove("default");
            }
        }
    }
}

/// Also leaves out the largest values left until `fits` says the response
/// fits [`MAX_DISCOVERY_RESPONSE_BYTES`](super::MAX_DISCOVERY_RESPONSE_BYTES): values each under the budget can
/// still add up past it.
pub(crate) fn omit_until_fits(
    controls: &[ControlMeta],
    values: &BTreeMap<String, ControlValue>,
    omitted: &mut BTreeMap<String, OmittedValue>,
    mut fits: impl FnMut(&BTreeMap<String, OmittedValue>) -> bool,
) {
    let mut sized: Vec<(usize, &String)> = (controls.iter())
        .filter(|meta| !omitted.contains_key(&meta.key))
        .map(|meta| {
            let value = values.get(&meta.key).map_or(0, json_bytes);
            (json_bytes(&meta.default).max(value), &meta.key)
        })
        .collect();
    sized.sort_unstable_by(|a, b| b.cmp(a));
    for (bytes, key) in sized {
        if fits(omitted) {
            return;
        }
        let value = OmittedValue {
            bytes: bytes as u64,
            cell_lengths: None,
        };
        omitted.insert(key.clone(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow::Borrowed;

    fn meta(key: &str, default: ControlValue) -> ControlMeta {
        ControlMeta::string(key, "").with_default(default)
    }

    fn cells() -> StoredCells {
        StoredCells {
            cell_key: Borrowed("cells"),
            max_cells: 64,
            playing: Borrowed("cell"),
            select: Borrowed("select_cell"),
            start: Borrowed("start_cell"),
            mode: Borrowed("mode"),
            follow_length: Borrowed("follow_cell_length"),
        }
    }

    #[test]
    fn a_value_past_the_budget_is_named_with_its_size_and_its_cells() {
        let bank = format!("[[{}],[null,null]]", vec!["null"; 2000].join(","));
        let controls = [
            meta("cells", bank.as_str().into()),
            meta("mode", "loop".into()),
        ];
        let values = BTreeMap::from([("cells".to_string(), bank.as_str().into())]);
        let omitted = omitted_values(&controls, &values, Some(&cells()));
        assert_eq!(omitted.len(), 1);
        assert_eq!(omitted["cells"].bytes, json_bytes(&bank) as u64);
        assert_eq!(omitted["cells"].cell_lengths, Some(vec![2000, 2]));
    }

    #[test]
    fn values_under_the_budget_that_add_up_past_it_are_left_out_largest_first() {
        let sized = |n: usize| ControlValue::String("x".repeat(n));
        let controls = [
            meta("a", sized(7000)),
            meta("b", sized(6000)),
            meta("c", sized(10)),
        ];
        let mut omitted = BTreeMap::new();
        omit_until_fits(&controls, &BTreeMap::new(), &mut omitted, |o| o.len() >= 2);
        let keys: Vec<&str> = omitted.keys().map(String::as_str).collect();
        assert_eq!(keys, ["a", "b"]);
        assert_eq!(omitted["a"].bytes, 7002);
    }

    #[test]
    fn a_left_out_default_is_dropped_from_the_listing() {
        let omitted = BTreeMap::from([(
            "cells".to_string(),
            OmittedValue {
                bytes: 9000,
                cell_lengths: None,
            },
        )]);
        let mut listed = serde_json::json!([
            {"key": "cells", "default": ""},
            {"key": "mode", "default": "loop"},
        ]);
        leave_out_defaults(&mut listed, &omitted);
        assert_eq!(
            listed,
            serde_json::json!([{"key": "cells"}, {"key": "mode", "default": "loop"}])
        );
    }
}
