//! Control values too large to describe. A module's description and its
//! listed controls carry each control's value (and, listed, its default,
//! which for a running instance is the same value). A value larger than
//! [`MAX_DESCRIBED_VALUE_BYTES`], such as a cell sequencer's whole bank of
//! cells, is left out of both and named in `values_omitted` with its size,
//! so a description never fails for size because of one control. A reader
//! takes the value through `get_control`, whole or, for a bank of cells,
//! one cell at a time as `cells.N`.

use super::discovery::{check_discovery_size, MetadataSource, ModuleTypeInfo, RegistryScope};
use super::{ModuleDescription, RpcError};
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
/// [`ControlMeta`], whose value is omitted. Read `controls` from JSON text
/// rather than with `serde_json::to_value`, which widens an `f32`.
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

/// The bytes a description saves by leaving out control `key`: its entry
/// in `control_values` when a value was read, of `value` bytes, and its
/// listed `"default"` of `default` bytes, less its entry in
/// `values_omitted` naming `bytes`.
fn saved_bytes(key: &str, value: Option<usize>, default: usize, bytes: usize) -> usize {
    let key = json_bytes(&key);
    let entry = |value: usize| key + 1 + value + 1; // "key":value,
    let removed = value.map_or(0, entry) + r#","default":"#.len() + default;
    let named = OmittedValue {
        bytes: bytes as u64,
        cell_lengths: None,
    };
    removed.saturating_sub(entry(json_bytes(&named)))
}

/// Also leaves out the largest values left until `fits` says the response
/// fits [`MAX_DISCOVERY_RESPONSE_BYTES`](super::MAX_DISCOVERY_RESPONSE_BYTES):
/// values each under the budget can still add up past it. Leaves out as few
/// as fit, the largest savings first, found by halving, so a large control surface costs a few trials;
/// when the response does not fit with every value left out, leaves
/// `omitted` as it is.
pub(crate) fn omit_until_fits(
    controls: &[ControlMeta],
    values: &BTreeMap<String, ControlValue>,
    omitted: &mut BTreeMap<String, OmittedValue>,
    mut fits: impl FnMut(&BTreeMap<String, OmittedValue>) -> bool,
) {
    if fits(omitted) {
        return;
    }
    // Each control by the bytes leaving it out saves, largest first; one
    // that saves none (a small default with no value read) stays.
    let mut sized: Vec<(usize, usize, &String)> = (controls.iter())
        .filter(|meta| !omitted.contains_key(&meta.key))
        .filter_map(|meta| {
            let value = values.get(&meta.key).map(json_bytes);
            let default = json_bytes(&meta.default);
            let bytes = default.max(value.unwrap_or(0));
            let saved = saved_bytes(&meta.key, value, default, bytes);
            (saved > 0).then_some((saved, bytes, &meta.key))
        })
        .collect();
    sized.sort_unstable_by(|a, b| b.cmp(a));
    let leaving = |count: usize| {
        let mut leaving = omitted.clone();
        for &(_, bytes, key) in &sized[..count] {
            let value = OmittedValue {
                bytes: bytes as u64,
                cell_lengths: None,
            };
            leaving.insert(key.clone(), value);
        }
        leaving
    };
    // The fewest largest values whose leaving out fits; every one, checked
    // first, is the most there is to leave.
    let (mut low, mut high) = (1, sized.len());
    if high == 0 || !fits(&leaving(high)) {
        return;
    }
    while low < high {
        let middle = low + (high - low) / 2;
        if fits(&leaving(middle)) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    *omitted = leaving(high);
}

/// [`ModuleDescription`] as it serializes before an omitted value's listed
/// default is dropped.
#[derive(Serialize)]
#[serde(remote = "ModuleDescription")]
struct DescriptionFields {
    schema_version: u32,
    registry_scope: RegistryScope,
    metadata_source: MetadataSource,
    sample_rate: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    module_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(flatten)]
    info: ModuleTypeInfo,
    control_values: BTreeMap<String, ControlValue>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    values_omitted: BTreeMap<String, OmittedValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeline: Option<crate::TimelineContext>,
}

impl Serialize for ModuleDescription {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.values_omitted.is_empty() {
            return DescriptionFields::serialize(self, serializer);
        }
        use serde::ser::Error;
        // Through JSON text, not straight to a `Value`, so an `f32` keeps the
        // shortest form it is written in (0.1, not 0.10000000149011612).
        struct Fields<'a>(&'a ModuleDescription);
        impl Serialize for Fields<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                DescriptionFields::serialize(self.0, s)
            }
        }
        let text = serde_json::to_string(&Fields(self)).map_err(S::Error::custom)?;
        let mut value: serde_json::Value = serde_json::from_str(&text).map_err(S::Error::custom)?;
        if let Some(controls) = value.get_mut("controls") {
            leave_out_defaults(controls, &self.values_omitted);
        }
        value.serialize(serializer)
    }
}

impl ModuleDescription {
    /// This description with each value too large to describe left out and
    /// named, and then, while it is still too large, the largest left; so
    /// it fails for size only when what is not a value is too large.
    pub(super) fn leaving_out_large_values(mut self) -> Result<Self, RpcError> {
        let cells = self.info.stored_notes.as_ref();
        let cells = cells.and_then(|notes| notes.cells.as_ref());
        let mut omitted = omitted_values(&self.info.controls, &self.control_values, cells);
        if !omitted.is_empty() || check_discovery_size(&self).is_err() {
            let (controls, values) = (self.info.controls.clone(), self.control_values.clone());
            omit_until_fits(&controls, &values, &mut omitted, |omitted| {
                let mut trial = self.clone();
                leave_out(&mut trial.info.controls, &mut trial.control_values, omitted);
                trial.values_omitted = omitted.clone();
                check_discovery_size(&trial).is_ok()
            });
            leave_out(&mut self.info.controls, &mut self.control_values, &omitted);
            self.values_omitted = omitted;
        }
        check_discovery_size(&self)?;
        Ok(self)
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
    fn a_response_that_cannot_fit_is_left_as_it_is_after_a_few_trials() {
        let controls: Vec<_> = (0..65_536)
            .map(|n| meta(&format!("slot.{n}"), "x".repeat(40).as_str().into()))
            .collect();
        let named = BTreeMap::from([(
            "cells".to_string(),
            OmittedValue {
                bytes: 9000,
                cell_lengths: None,
            },
        )]);
        let mut omitted = named.clone();
        let mut trials = 0;
        omit_until_fits(&controls, &BTreeMap::new(), &mut omitted, |_| {
            trials += 1;
            false
        });
        assert_eq!((trials, omitted), (2, named));

        // One that fits only with nearly all left out takes a trial per halving.
        let mut omitted = BTreeMap::new();
        let mut trials = 0;
        omit_until_fits(&controls, &BTreeMap::new(), &mut omitted, |o| {
            trials += 1;
            o.len() >= 60_000
        });
        assert_eq!(omitted.len(), 60_000);
        assert!(trials <= 20, "{trials}");
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

    #[test]
    fn a_control_whose_leaving_out_saves_nothing_stays_listed() {
        // A large value and many controls with no value read: leaving those
        // out would only add their names, so the search leaves the large one.
        let mut controls = vec![meta("bank", "x".repeat(7000).as_str().into())];
        controls.extend((0..200).map(|n| meta(&format!("unread.{n}"), "".into())));
        let values = BTreeMap::from([("bank".to_string(), "x".repeat(7000).as_str().into())]);
        let size = |omitted: &BTreeMap<String, OmittedValue>| {
            let mut listed = controls.clone();
            let mut shown = values.clone();
            leave_out(&mut listed, &mut shown, omitted);
            let mut wire = serde_json::json!({"controls": listed, "control_values": shown});
            leave_out_defaults(&mut wire["controls"], omitted);
            wire["values_omitted"] = serde_json::to_value(omitted).unwrap();
            json_bytes(&wire)
        };
        let whole = size(&BTreeMap::new());
        let mut omitted = BTreeMap::new();
        omit_until_fits(&controls, &values, &mut omitted, |o| size(o) < whole - 7000);
        let keys: Vec<&str> = omitted.keys().map(String::as_str).collect();
        assert_eq!(keys, ["bank"]);
    }
}
