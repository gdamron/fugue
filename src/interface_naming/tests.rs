//! The naming rules, name by name, and the registry-wide guard.

use super::*;

mod exceptions;
mod guard;

#[test]
fn conforming_names_pass() {
    for name in [
        "audio_left",
        "level.0",
        "level.12",
        "beat_x2",
        "step_count",
        "wet",
    ] {
        assert_eq!(naming_problems(name), Vec::<String>::new(), "{name}");
    }
}

#[test]
fn each_rule_is_caught() {
    for name in [
        "Level",
        "level.01",
        "in1",
        "type",
        "cv_amount",
        "grace_duration_ms",
        "left",
        "is_on",
        "sample_end_gate",
        "gain.0",
        "history_json",
        "tick_hz",
    ] {
        assert!(!naming_problems(name).is_empty(), "{name}");
    }
}

#[test]
fn the_established_mix_words_are_amplitude_words() {
    for (word, _) in ESTABLISHED_MIX_WORDS {
        assert!(AMPLITUDE_WORDS.contains(word), "{word}");
    }
}
