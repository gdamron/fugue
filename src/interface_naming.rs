//! The naming convention for module interfaces: the inputs, outputs,
//! controls and config keys a module exposes (ledger
//! `module-interface-conventions`). Built-in modules are held to it by the
//! registry-wide convention test; a wasm guest's port names are checked as
//! it loads and warned about, never refused.

/// Segments a name never uses, with the word that replaces each.
const BANNED_SEGMENTS: &[(&str, &str)] = &[
    ("cv", "name the musical quantity, such as level"),
    ("json", "a structured value takes no _json suffix"),
    ("ms", "times are in seconds, with no suffix"),
    ("seconds", "times are in seconds, with no suffix"),
    ("hz", "frequencies are in Hz, with no suffix"),
];

/// Words for a linear amplitude, which is `level` or `<x>_level`.
const AMPLITUDE_WORDS: &[&str] = &["gain", "volume", "master", "wet", "dry"];

/// Amplitude words kept beside `level` (decision D6). Permanent: they are
/// the words musicians use for these mixes.
pub(crate) const ESTABLISHED_MIX_WORDS: &[(&str, &str)] = &[
    ("master", "the mix bus level"),
    ("wet", "the processed share of an effect's output"),
    ("dry", "the unprocessed share of an effect's output"),
];

/// Names a port never takes alone.
const BARE_NAMES: &[&str] = &["in", "out", "left", "right"];

/// Prefixes that make a boolean negative or a question.
const BOOLEAN_PREFIXES: &[&str] = &["is_", "no_", "enable_", "disable_"];

/// How `name` breaks the convention: one reason per broken rule, none when
/// it conforms. An indexed name is `stem.N`, N from 0 with no leading zero,
/// or `stem.N` itself for a declared family.
pub(crate) fn naming_problems(name: &str) -> Vec<String> {
    let (stem, index) = match name.split_once('.') {
        Some((stem, index)) => (stem, Some(index)),
        None => (name, None),
    };
    let lower = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let segments: Vec<&str> = stem.split('_').collect();
    let mut problems = Vec::new();
    let spelled = stem.starts_with(|c: char| c.is_ascii_lowercase())
        && segments
            .iter()
            .all(|s| !s.is_empty() && s.chars().all(lower))
        && index.is_none_or(|n| n == "0" || n == "N" || (!n.starts_with('0') && is_digits(n)));
    if !spelled {
        problems.push("is not lower snake_case with an optional .N index".to_string());
    }
    if segments.iter().any(|segment| glued_digits(segment)) {
        problems.push("glues digits onto a word; an index is name.N".to_string());
    }
    if stem == "type" {
        problems.push("is 'type'; name the dimension (waveform, filter_type)".to_string());
    }
    for (banned, instead) in BANNED_SEGMENTS {
        if segments.contains(banned) {
            problems.push(format!("uses '{banned}': {instead}"));
        }
    }
    let established = |word: &&str| ESTABLISHED_MIX_WORDS.iter().any(|(w, _)| w == word);
    for word in AMPLITUDE_WORDS.iter().filter(|w| !established(w)) {
        if segments.contains(word) {
            problems.push(format!("uses '{word}': linear amplitude is level"));
        }
    }
    if BARE_NAMES.contains(&stem) {
        problems.push("is a bare direction; audio is audio or audio_left/audio_right".into());
    }
    if let Some(prefix) = BOOLEAN_PREFIXES.iter().find(|p| stem.starts_with(*p)) {
        problems.push(format!(
            "starts with '{prefix}'; a boolean names the on state"
        ));
    }
    if stem.ends_with("_gate") {
        problems.push("ends in _gate; a trigger output names its moment".to_string());
    }
    problems
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_digit())
}

/// A word with digits after it (`in1`), other than a rate multiplier or
/// divisor such as `x2` or `d4` (decision D7).
fn glued_digits(segment: &str) -> bool {
    let word = segment.trim_end_matches(|c: char| c.is_ascii_digit());
    let rate = (word == "x" || word == "d") && word.len() < segment.len();
    !word.is_empty() && word.len() < segment.len() && !rate
}

#[cfg(test)]
mod tests;
