//! Names that break the interface convention today, each until its module
//! family is renamed. Seeded from the convention spec's audit table.
//!
//! The list only shrinks. A family's rename deletes its block below, and
//! the guard fails on a row that no longer excuses anything, so a block
//! cannot outlive its rename. A broken rule is fixed by renaming, never by
//! a new row. A row is (type, name, the rename that retires it); a name is
//! filed as the guard reports it: an indexed family as `level.N`, digits
//! glued onto a word as `in<N>`, and the whole config as `*`.

/// The list's seeded length. Never raise it.
pub(super) const EXCEPTION_CEILING: usize = 40;

// One row per line, so a rename deletes whole lines.
#[rustfmt::skip]
pub(super) const CONVENTION_EXCEPTIONS: &[(&str, &str, &str)] = &[
    // filter and reverb
    ("filter", "type", "control becomes filter_type, the config's name"),
    ("filter", "filter_type", "matches the control once it is filter_type"),
    ("filter", "cutoff_cv", "input becomes cutoff_mod"),
    ("filter", "cv_amount", "becomes cutoff_mod_depth"),
    ("reverb", "left", "ports become audio_left"),
    ("reverb", "right", "ports become audio_right"),
    // mixer and divisi
    ("mixer", "in<N>", "inputs become audio.N, from 0"),
    ("mixer", "level<N>", "inputs become level.N, from 0, the controls' names"),
    ("mixer", "pan<N>", "inputs become pan.N, from 0, the controls' names"),
    ("mixer", "left", "output becomes audio_left"),
    ("mixer", "right", "output becomes audio_right"),
    ("divisi", "frequency<N>", "outputs become frequency.N, from 0"),
    ("divisi", "gate<N>", "outputs become gate.N, from 0"),
    ("divisi", "velocity<N>", "outputs become velocity.N, from 0"),
    // step_sequencer and melody
    ("step_sequencer", "grace_duration_ms", "becomes grace_duration, in seconds"),
    ("step_sequencer", "pattern_json", "control becomes pattern; the config key goes"),
    ("step_sequencer", "pattern", "matches the control once it is pattern"),
    ("melody", "scale_degrees", "config becomes degrees"),
    // cell_sequencer
    ("cell_sequencer", "grace_duration_ms", "becomes grace_duration, in seconds"),
    ("cell_sequencer", "sequences_json", "control becomes cells; the config key goes"),
    ("cell_sequencer", "sequences", "config becomes cells"),
    // sample_player and sample_slicer
    ("sample_player", "loop_enabled", "config becomes loop, the control's name"),
    ("sample_player", "sample_start_gate", "output becomes start"),
    ("sample_player", "sample_end_gate", "output becomes end"),
    ("sample_slicer", "slice_start_gate", "output becomes start"),
    ("sample_slicer", "slice_end_gate", "output becomes end"),
    // sample_kit and sample_instrument
    ("sample_kit", "gain.N", "controls become level.N"),
    ("sample_instrument", "gain.N", "controls become level.N"),
    // agent, code and the sinks
    ("agent", "cooldown_ms", "becomes cooldown, in seconds"),
    ("agent", "history_json", "telemetry becomes history"),
    ("agent", "history", "config's history settings clash with the history telemetry"),
    ("agent", "last_response_json", "telemetry becomes last_parsed_response"),
    ("code", "tick_hz", "becomes tick_rate"),
    ("code", "*", "open until script parameters move under params"),
    ("rtmp_sink", "gop_seconds", "becomes gop_duration"),
    ("youtube_sink", "gop_seconds", "becomes gop_duration"),
];
