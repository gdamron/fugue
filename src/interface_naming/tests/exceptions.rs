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
