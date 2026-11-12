//! Human names a module type declares for itself and what it offers.
//!
//! Surfaces name things in layers: what the piece calls a module (its
//! `name`), then what its type declares here, then the id tidied by
//! [`tidy_key`]. A type declares a label only where tidying says it badly
//! ("lowpass" is better as "Low-pass", "pw" as "Pulse width"), and a label
//! is listed only when it differs from the tidied key, so a listing grows
//! only by what it adds. Labels are presentation: nothing addresses a
//! module, control, value or port by its label.

use std::collections::BTreeMap;

/// Short forms a musician reads in capitals, as a key writes them in
/// lowercase.
const SPELLED: &[(&str, &str)] = &[
    ("adsr", "ADSR"),
    ("am", "AM"),
    ("bp", "BP"),
    ("bpm", "BPM"),
    ("cc", "CC"),
    ("cv", "CV"),
    ("db", "dB"),
    ("dc", "DC"),
    ("eq", "EQ"),
    ("fm", "FM"),
    ("hp", "HP"),
    ("hz", "Hz"),
    ("lfo", "LFO"),
    ("lp", "LP"),
    ("midi", "MIDI"),
    ("pm", "PM"),
    ("rms", "RMS"),
];

/// A key in words, as every surface tidies an id it has no label for:
/// `root_note` is "Root note", `filterHPCutoff` "Filter HP cutoff",
/// `level.0` "Level 0" (an index stays as the key has it), `bpm` "BPM".
/// Words split at `.`, `_`, `-` and spaces, and where a lowercase letter or
/// digit meets a capital; a run of capitals stays one word, as written.
pub fn tidy_key(key: &str) -> String {
    let words: Vec<String> = key
        .split(['.', '_', '-', ' ', '\t', '\n'])
        .flat_map(split_case)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let capitals = part.chars().filter(|c| c.is_ascii_uppercase()).count();
            if part.chars().count() > 1 && capitals > 0 && part == part.to_uppercase() {
                return part.to_string();
            }
            let lower = part.to_lowercase();
            spelled(&lower).map_or(lower, str::to_string)
        })
        .collect();
    if words.is_empty() {
        return key.to_string();
    }
    let name = words.join(" ");
    // A short form keeps its own spelling: "dB range", not "DB range".
    if SPELLED.iter().any(|(_, short)| *short == words[0]) {
        return name;
    }
    let mut chars = name.chars();
    let first = chars.next().map(|c| c.to_uppercase().to_string());
    first.unwrap_or_default() + chars.as_str()
}

fn spelled(lower: &str) -> Option<&'static str> {
    SPELLED
        .iter()
        .find(|(key, _)| *key == lower)
        .map(|(_, short)| *short)
}

/// Splits `part` where a lowercase letter or digit meets a capital, and
/// before the last capital of a run followed by a lowercase letter
/// (`filterHPCutoff` is `filter`, `HP`, `Cutoff`).
fn split_case(part: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = part.char_indices().collect();
    let mut pieces = Vec::new();
    let mut start = 0;
    for window in 1..chars.len() {
        let (index, current) = chars[window];
        let previous = chars[window - 1].1;
        let next = chars.get(window + 1).map(|(_, c)| *c);
        let lower_to_upper = (previous.is_ascii_lowercase() || previous.is_ascii_digit())
            && current.is_ascii_uppercase();
        let run_end = previous.is_ascii_uppercase()
            && current.is_ascii_uppercase()
            && next.is_some_and(|c| c.is_ascii_lowercase());
        if lower_to_upper || run_end {
            pieces.push(&part[start..index]);
            start = index;
        }
    }
    pieces.push(&part[start..]);
    pieces
}

/// `label` as listed for `key`: `None` when it says no more than the
/// tidied key.
pub(crate) fn listed_label(key: &str, label: Option<&str>) -> Option<String> {
    let label = label?;
    (label != tidy_key(key)).then(|| label.to_string())
}

/// The labels of `pairs` (each a name and its label) that differ from the
/// tidied name, by name.
pub(crate) fn listed_labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    (pairs.iter())
        .filter_map(|(name, label)| Some((name.to_string(), listed_label(name, Some(label))?)))
        .collect()
}

/// The labels a module type declares for itself and its ports (see the
/// module docs). Each port label pairs a port's name with its label.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TypeLabels {
    /// What the type is called ("Filter", "Clock"), when tidying its type
    /// name says it badly.
    pub module: Option<&'static str>,
    /// Labels for input ports, by port name.
    pub inputs: &'static [(&'static str, &'static str)],
    /// Labels for output ports, by port name.
    pub outputs: &'static [(&'static str, &'static str)],
}

impl TypeLabels {
    /// No labels: every name is tidied.
    pub const NONE: Self = Self {
        module: None,
        inputs: &[],
        outputs: &[],
    };
}

#[cfg(test)]
#[path = "labels_tests.rs"]
mod tests;
