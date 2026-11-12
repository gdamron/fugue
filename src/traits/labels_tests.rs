use super::*;

#[test]
fn keys_tidy_as_surfaces_name_them() {
    let cases = [
        ("root_note", "Root note"),
        ("cutoff", "Cutoff"),
        ("level.0", "Level 0"),
        ("filter_type", "Filter type"),
        ("filterHPCutoff", "Filter HP cutoff"),
        ("bpm", "BPM"),
        ("db_range", "dB range"),
        ("lfo_rate", "LFO rate"),
        ("cutoff_mod_depth", "Cutoff mod depth"),
        ("audio_left", "Audio left"),
        ("lowpass", "Lowpass"),
        ("12db", "12db"),
        ("fm-amount", "FM amount"),
        ("a", "A"),
        ("", ""),
        ("_", "_"),
    ];
    for (key, tidied) in cases {
        assert_eq!(tidy_key(key), tidied, "{key}");
    }
}

#[test]
fn a_label_is_listed_only_when_it_says_more_than_the_tidied_key() {
    assert_eq!(listed_label("cutoff", Some("Cutoff")), None);
    assert_eq!(listed_label("cutoff", None), None);
    assert_eq!(
        listed_label("lowpass", Some("Low-pass")).as_deref(),
        Some("Low-pass")
    );
    let labels = listed_labels(&[("audio", "Audio"), ("cv", "Cutoff CV")]);
    assert_eq!(labels.len(), 1);
    assert_eq!(labels["cv"], "Cutoff CV");
}
