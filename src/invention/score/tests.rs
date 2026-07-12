use super::*;
use serde_json::json;

/// The shipped In C score asset must validate against the schema it generalizes.
#[test]
fn in_c_score_asset_validates() {
    let json = include_str!("../../../examples/in_c/score.json");
    let value: Value = serde_json::from_str(json).expect("in_c score.json parses");
    validate_score(&value).expect("in_c score.json validates");

    let score = Score::from_json(json).expect("in_c score.json deserializes");
    assert_eq!(score.base_note_hint, Some(60));
    assert_eq!(score.rhythm_grid.as_deref(), Some("32nd_note"));
    assert!(!score.cells.is_empty());
}

#[test]
fn full_metadata_score_validates() {
    let value = json!({
        "schema": "fugue.score.v1",
        "title": "The Flow of Water",
        "composer": "Grant Damron",
        "key": "Ab major",
        "tempo": 120.0,
        "time_signature": { "beats_per_measure": 4, "beat_unit": 4 },
        "base_note_hint": 48,
        "rhythm_grid": "16th_note",
        "cells": [
            [ { "note": 0 }, { "held": true }, { "note": 7, "gate": 0.8 }, null ],
            [ 4, { "note": null }, { "note": -12 } ]
        ]
    });
    validate_score(&value).expect("full metadata score validates");
}

/// A through-composed / single-sequence piece is just a bank of one cell.
#[test]
fn single_cell_score_validates() {
    let value = json!({ "cells": [[ { "note": 0 }, null, 5, { "held": true } ]] });
    validate_score(&value).expect("single-cell score validates");
}

#[test]
fn round_trips_through_typed_model() {
    let value = json!({
        "schema": "fugue.score.v1",
        "title": "Round Trip",
        "base_note_hint": 60,
        "cells": [[ { "note": 0 }, { "note": 7, "gate": 0.5 }, { "held": true }, null ]]
    });
    let json = serde_json::to_string(&value).unwrap();
    let score = Score::from_json(&json).expect("typed parse");
    let reserialized = score.to_json().expect("serialize");
    validate_score(&serde_json::from_str::<Value>(&reserialized).unwrap())
        .expect("re-serialized score still validates");
}

#[test]
fn rejects_non_object() {
    let err = validate_score(&json!([1, 2, 3])).unwrap_err();
    assert!(err.contains("must be a JSON object"), "{err}");
}

#[test]
fn rejects_unknown_schema_version() {
    let value = json!({ "schema": "fugue.score.v2", "cells": [[ { "note": 0 } ]] });
    let err = validate_score(&value).unwrap_err();
    assert!(err.contains("unsupported score schema"), "{err}");
}

#[test]
fn rejects_missing_cells() {
    let value = json!({ "title": "Empty", "base_note_hint": 60 });
    let err = validate_score(&value).unwrap_err();
    assert!(err.contains("non-empty 'cells' array"), "{err}");
}

#[test]
fn rejects_empty_cells() {
    let err = validate_score(&json!({ "cells": [] })).unwrap_err();
    assert!(err.contains("cells must not be empty"), "{err}");
}

#[test]
fn rejects_empty_inner_cell() {
    let err = validate_score(&json!({ "cells": [[]] })).unwrap_err();
    assert!(err.contains("cells[0] must not be empty"), "{err}");
}

#[test]
fn rejects_non_integer_note() {
    let err = validate_score(&json!({ "cells": [[ { "note": "C4" } ]] })).unwrap_err();
    assert!(
        err.contains("step.note must be an integer or null"),
        "{err}"
    );
}

#[test]
fn rejects_out_of_range_note() {
    let err = validate_score(&json!({ "cells": [[ { "note": 500 } ]] })).unwrap_err();
    assert!(err.contains("out of range"), "{err}");
}

#[test]
fn rejects_out_of_range_base_note() {
    let err = validate_score(&json!({ "base_note_hint": 200, "cells": [[ 0 ]] })).unwrap_err();
    assert!(
        err.contains("base_note_hint must be between 0 and 127"),
        "{err}"
    );
}

#[test]
fn rejects_gate_above_one() {
    let err = validate_score(&json!({ "cells": [[ { "note": 0, "gate": 1.5 } ]] })).unwrap_err();
    assert!(err.contains("step.gate must be between 0 and 1"), "{err}");
}

#[test]
fn rejects_amplitude_above_one() {
    let err =
        validate_score(&json!({ "cells": [[ { "note": 0, "amplitude": 1.5 } ]] })).unwrap_err();
    assert!(
        err.contains("step.amplitude must be between 0 and 1"),
        "{err}"
    );
}

#[test]
fn rejects_non_numeric_amplitude() {
    let err =
        validate_score(&json!({ "cells": [[ { "note": 0, "amplitude": "f" } ]] })).unwrap_err();
    assert!(err.contains("step.amplitude must be a number"), "{err}");
}

#[test]
fn accepts_amplitude_on_note_steps() {
    validate_score(&json!({ "cells": [[ { "note": 0, "amplitude": 0.5 } ]] }))
        .expect("amplitude in range must validate");
}

#[test]
fn rejects_held_with_extra_keys() {
    let value = json!({ "cells": [[ { "held": true, "note": 0 } ]] });
    let err = validate_score(&value).unwrap_err();
    assert!(err.contains("held steps may only contain"), "{err}");
}

#[test]
fn rejects_non_positive_tempo() {
    let err = validate_score(&json!({ "tempo": 0.0, "cells": [[ 0 ]] })).unwrap_err();
    assert!(err.contains("tempo must be a positive number"), "{err}");
}

#[test]
fn rejects_zero_beat_unit() {
    let value = json!({
        "time_signature": { "beats_per_measure": 4, "beat_unit": 0 },
        "cells": [[ 0 ]]
    });
    let err = validate_score(&value).unwrap_err();
    assert!(err.contains("beat_unit must be greater than 0"), "{err}");
}

#[test]
fn cell_error_reports_index() {
    let value = json!({ "cells": [ [ { "note": 0 } ], [ { "note": "x" } ] ] });
    let err = validate_score(&value).unwrap_err();
    assert!(err.contains("cells[1]"), "{err}");
}

#[test]
fn tempo_map_validates_and_round_trips() {
    let value = json!({
        "schema": "fugue.score.v1",
        "tempo": 60.0,
        "tempo_map": [
            { "at_step": 0, "bpm": 60.0 },
            { "at_step": 256, "bpm": 124.0 },
            { "at_step": 528, "bpm": 62.0 }
        ],
        "cells": [[ 0 ]]
    });
    validate_score(&value).expect("tempo_map validates");

    let score = Score::from_json(&value.to_string()).expect("typed parse");
    assert_eq!(score.tempo_map.len(), 3);
    assert_eq!(score.tempo_map[1].at_step, 256);
    assert_eq!(score.tempo_map[1].bpm, 124.0);

    // A constant-tempo score omits the map entirely and stays valid (v1 compat).
    let constant = Score::from_json(&json!({ "tempo": 90.0, "cells": [[ 0 ]] }).to_string())
        .expect("constant-tempo score parses");
    assert!(constant.tempo_map.is_empty());
    let reserialized: Value = serde_json::from_str(&constant.to_json().unwrap()).unwrap();
    assert!(
        reserialized.get("tempo_map").is_none(),
        "an empty tempo_map is not serialized"
    );
}

#[test]
fn rejects_unordered_tempo_map() {
    let value = json!({
        "tempo_map": [ { "at_step": 10, "bpm": 60.0 }, { "at_step": 10, "bpm": 90.0 } ],
        "cells": [[ 0 ]]
    });
    let err = validate_score(&value).unwrap_err();
    assert!(
        err.contains("must be greater than the previous entry"),
        "{err}"
    );
}

#[test]
fn tempo_map_ramp_validates_and_round_trips() {
    let value = json!({
        "tempo_map": [
            { "at_step": 0, "bpm": 60.0 },
            { "at_step": 100, "bpm": 40.0, "ramp": 32 }
        ],
        "cells": [[ 0 ]]
    });
    validate_score(&value).expect("ramped tempo_map validates");
    let score = Score::from_json(&value.to_string()).expect("parses");
    assert_eq!(score.tempo_map[1].ramp, Some(32));
    // A no-ramp entry omits the field on re-serialization.
    let reserialized: Value = serde_json::from_str(&score.to_json().unwrap()).unwrap();
    assert!(reserialized["tempo_map"][0].get("ramp").is_none());
}

#[test]
fn rejects_zero_ramp_in_tempo_map() {
    let value = json!({
        "tempo_map": [ { "at_step": 0, "bpm": 60.0, "ramp": 0 } ],
        "cells": [[ 0 ]]
    });
    let err = validate_score(&value).unwrap_err();
    assert!(err.contains("ramp must be at least 1 step"), "{err}");
}

#[test]
fn rejects_non_positive_tempo_map_bpm() {
    let value = json!({
        "tempo_map": [ { "at_step": 0, "bpm": 0.0 } ],
        "cells": [[ 0 ]]
    });
    let err = validate_score(&value).unwrap_err();
    assert!(
        err.contains("tempo_map[0].bpm must be a positive number"),
        "{err}"
    );
}

#[test]
fn accepts_pedal_lanes() {
    validate_score(&json!({
        "cells": [[0, null]],
        "pedal": [[ { "note": 0 }, { "held": true } ]]
    }))
    .unwrap();
}

#[test]
fn rejects_empty_pedal_lane() {
    let err = validate_score(&json!({ "cells": [[0]], "pedal": [[]] })).unwrap_err();
    assert!(err.contains("pedal[0] must not be empty"), "{err}");
}

#[test]
fn rejects_malformed_pedal_step() {
    let err = validate_score(&json!({
        "cells": [[0]],
        "pedal": [[ { "note": "down" } ]]
    }))
    .unwrap_err();
    assert!(err.contains("pedal[0]"), "{err}");
}

#[test]
fn accepts_grace_on_note_steps() {
    let value = json!({ "cells": [[ { "note": 24, "grace": [12] }, { "held": true } ]] });
    validate_score(&value).expect("grace on a note step validates");

    let value = json!({ "cells": [[ { "note": 0, "grace": [-1, -3, -1, 0] } ]] });
    validate_score(&value).expect("a four-note grace chain validates");
}

#[test]
fn rejects_grace_on_rests_and_held_steps() {
    let err =
        validate_score(&json!({ "cells": [[ { "note": null, "grace": [5] } ]] })).unwrap_err();
    assert!(err.contains("requires a principal note"), "{err}");

    let err = validate_score(&json!({ "cells": [[ { "grace": [5] } ]] })).unwrap_err();
    assert!(err.contains("requires a principal note"), "{err}");

    // Held steps may carry nothing else, so the existing held rule rejects it.
    let err =
        validate_score(&json!({ "cells": [[ { "note": 0 }, { "held": true, "grace": [5] } ]] }))
            .unwrap_err();
    assert!(err.contains("held"), "{err}");
}

#[test]
fn rejects_malformed_grace() {
    let err = validate_score(&json!({ "cells": [[ { "note": 0, "grace": [] } ]] })).unwrap_err();
    assert!(err.contains("must not be empty"), "{err}");

    let err = validate_score(&json!({ "cells": [[ { "note": 0, "grace": [1, 2, 3, 4, 5] } ]] }))
        .unwrap_err();
    assert!(err.contains("at most 4"), "{err}");

    let err = validate_score(&json!({ "cells": [[ { "note": 0, "grace": [1.5] } ]] })).unwrap_err();
    assert!(err.contains("must be integers"), "{err}");

    let err = validate_score(&json!({ "cells": [[ { "note": 0, "grace": [200] } ]] })).unwrap_err();
    assert!(err.contains("out of range"), "{err}");

    // The object form is reserved for future ornament kinds (trills etc.).
    let err =
        validate_score(&json!({ "cells": [[ { "note": 0, "grace": { "kind": "trill" } } ]] }))
            .unwrap_err();
    assert!(err.contains("reserved"), "{err}");
}

#[test]
fn grace_round_trips_through_typed_model() {
    let value = json!({
        "schema": "fugue.score.v1",
        "cells": [[ { "note": 34, "grace": [22], "amplitude": 0.5 }, { "held": true } ]]
    });
    let json = serde_json::to_string(&value).unwrap();
    let score = Score::from_json(&json).expect("typed parse");
    assert_eq!(score.cells[0][0].grace.iter().collect::<Vec<_>>(), vec![22]);

    let reserialized = score.to_json().expect("serialize");
    let reparsed: Value = serde_json::from_str(&reserialized).unwrap();
    validate_score(&reparsed).expect("re-serialized score still validates");
    assert_eq!(reparsed["cells"][0][0]["grace"], json!([22]));
}
