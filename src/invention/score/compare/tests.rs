use super::*;
use crate::invention::TimeSignature;
use crate::modules::Step;

/// Builds a score on a 16th grid from cells of (offset, length-in-steps)
/// events; `None` entries are rests of the given length.
fn score(cells: Vec<Vec<(Option<i8>, usize)>>) -> Score {
    let cells = cells
        .into_iter()
        .map(|cell| {
            let mut steps = Vec::new();
            for (note, length) in cell {
                match note {
                    Some(offset) => {
                        steps.push(Step::note(offset));
                        for _ in 1..length {
                            steps.push(Step::held());
                        }
                    }
                    None => {
                        for _ in 0..length {
                            steps.push(Step::rest());
                        }
                    }
                }
            }
            steps
        })
        .collect();
    Score {
        schema: Some(crate::invention::score::SCORE_SCHEMA_V1.to_string()),
        base_note_hint: Some(60),
        rhythm_grid: Some("16th_note".to_string()),
        time_signature: Some(TimeSignature {
            beats_per_measure: 4,
            beat_unit: 4,
        }),
        ..Default::default()
    }
    .with_cells(cells)
}

trait WithCells {
    fn with_cells(self, cells: Vec<Vec<Step>>) -> Score;
}

impl WithCells for Score {
    fn with_cells(mut self, cells: Vec<Vec<Step>>) -> Score {
        self.cells = cells;
        self
    }
}

#[test]
fn identical_scores_are_exact() {
    let reference = score(vec![vec![(Some(0), 4), (None, 4), (Some(7), 8)]]);
    let report = compare_scores(&reference, &reference).expect("comparable");
    assert!(report.exact);
    assert_eq!(report.matched, 2);
    assert_eq!(report.f1, 1.0);
    assert_eq!(report.duration_accuracy, 1.0);
    assert!(report.total_duration_match);
    assert_eq!(report.reference_measures, Some(1));
}

#[test]
fn different_lane_decompositions_compare_equal() {
    // One polyphonic pair split across two cells vs. the same notes split
    // differently: (C4 whole, E4 whole) as two lanes, vs. lanes swapped.
    let reference = score(vec![vec![(Some(0), 16)], vec![(Some(4), 16)]]);
    let candidate = score(vec![vec![(Some(4), 16)], vec![(Some(0), 16)]]);
    let report = compare_scores(&candidate, &reference).expect("comparable");
    assert!(report.exact, "lane order must not matter: {:?}", report);
}

#[test]
fn different_rhythm_grids_align_on_the_time_base() {
    // The same quarter-note C4 expressed on a 16th grid and an 8th grid.
    let mut fine = score(vec![vec![(Some(0), 4), (Some(2), 4)]]);
    fine.rhythm_grid = Some("16th_note".to_string());
    let mut coarse = score(vec![vec![(Some(0), 2), (Some(2), 2)]]);
    coarse.rhythm_grid = Some("8th_note".to_string());
    let report = compare_scores(&coarse, &fine).expect("comparable");
    assert_eq!(report.matched, 2);
    assert!(report.exact, "{:?}", report);
}

#[test]
fn missing_and_extra_notes_hit_recall_and_precision() {
    let reference = score(vec![vec![
        (Some(0), 4),
        (Some(2), 4),
        (Some(4), 4),
        (None, 4),
    ]]);
    // Candidate drops E4 (offset 4) and adds a spurious B4 (offset 11).
    let candidate = score(vec![vec![
        (Some(0), 4),
        (Some(2), 4),
        (None, 4),
        (Some(11), 4),
    ]]);
    let report = compare_scores(&candidate, &reference).expect("comparable");
    assert_eq!(report.matched, 2);
    assert_eq!(report.reference_only, 1);
    assert_eq!(report.candidate_only, 1);
    assert!((report.precision - 2.0 / 3.0).abs() < 1e-9);
    assert!((report.recall - 2.0 / 3.0).abs() < 1e-9);
    assert!(!report.exact);
}

#[test]
fn dropped_tie_becomes_a_duration_mismatch_plus_extra_onset() {
    // Reference: C4 held for a half note. Candidate: two quarter onsets
    // (the classic dropped-tie transcription error).
    let reference = score(vec![vec![(Some(0), 8)]]);
    let candidate = score(vec![vec![(Some(0), 4), (Some(0), 4)]]);
    let report = compare_scores(&candidate, &reference).expect("comparable");
    assert_eq!(report.matched, 1, "the onset at beat 1 still matches");
    assert_eq!(report.duration_matches, 0, "but its duration is wrong");
    assert_eq!(
        report.candidate_only, 1,
        "the re-attack is a false positive"
    );
    assert!(!report.exact);
}

#[test]
fn octave_and_timing_errors_are_classified() {
    let reference = score(vec![vec![(Some(0), 4), (None, 4), (Some(7), 4), (None, 4)]]);
    // C4 transcribed an octave up; G4 transcribed one 16th late.
    let candidate = score(vec![vec![
        (Some(12), 4),
        (None, 4),
        (None, 1),
        (Some(7), 4),
        (None, 3),
    ]]);
    let report = compare_scores(&candidate, &reference).expect("comparable");
    assert_eq!(report.matched, 0);
    assert_eq!(report.octave_errors, 1);
    assert_eq!(report.timing_near_misses, 1);
}

#[test]
fn total_duration_mismatch_is_reported() {
    let reference = score(vec![vec![(Some(0), 16)]]);
    let candidate = score(vec![vec![(Some(0), 16), (None, 16)]]);
    let report = compare_scores(&candidate, &reference).expect("comparable");
    assert!(!report.total_duration_match);
    assert_eq!(report.candidate_measures, Some(2));
    assert_eq!(report.reference_measures, Some(1));
    assert!(!report.exact);
}

#[test]
fn base_note_hints_normalize_to_absolute_pitch() {
    // Same sounding note written relative to different base notes.
    let mut reference = score(vec![vec![(Some(0), 4)]]);
    reference.base_note_hint = Some(60);
    let mut candidate = score(vec![vec![(Some(-12), 4)]]);
    candidate.base_note_hint = Some(72);
    let report = compare_scores(&candidate, &reference).expect("comparable");
    assert!(report.exact, "{:?}", report);
}

#[test]
fn unknown_rhythm_grid_is_an_error() {
    let mut broken = score(vec![vec![(Some(0), 4)]]);
    broken.rhythm_grid = Some("vibes".to_string());
    let err = compare_scores(&broken, &score(vec![vec![(Some(0), 4)]]))
        .expect_err("unknown grid must fail");
    assert!(err.contains("rhythm_grid"), "{}", err);
}

#[test]
fn report_serializes_for_json_output() {
    let reference = score(vec![vec![(Some(0), 4)]]);
    let report = compare_scores(&reference, &reference).expect("comparable");
    let json = serde_json::to_value(&report).expect("serializes");
    assert_eq!(json["exact"], true);
    assert_eq!(json["reference_duration_quarters"], "1");
}

// --- Grace-aware matching (FUG-190) ---

/// Attaches a grace chain to the note step starting at `step_index` in
/// `cell_index`.
fn with_grace(mut score: Score, cell_index: usize, step_index: usize, grace: &[i8]) -> Score {
    score.cells[cell_index][step_index].grace =
        crate::modules::GraceChain::from_slice(grace).expect("chain fits");
    score
}

#[test]
fn matching_grace_chains_stay_exact() {
    let reference = with_grace(score(vec![vec![(Some(0), 4), (Some(34), 8)]]), 0, 4, &[22]);
    let candidate = with_grace(score(vec![vec![(Some(0), 4), (Some(34), 8)]]), 0, 4, &[22]);
    let report = compare_scores(&candidate, &reference).expect("comparable");
    assert!(report.exact);
    assert_eq!(report.candidate_grace_notes, 1);
    assert_eq!(report.reference_grace_notes, 1);
    assert_eq!(report.grace_matches, 1);
    assert_eq!(report.grace_mismatches, 0);
    assert_eq!(report.grace_accuracy, 1.0);
}

#[test]
fn grace_mismatch_breaks_exact_but_not_f1() {
    let reference = with_grace(score(vec![vec![(Some(0), 4), (Some(34), 8)]]), 0, 4, &[22]);
    // Same notes, wrong grace pitch.
    let candidate = with_grace(score(vec![vec![(Some(0), 4), (Some(34), 8)]]), 0, 4, &[24]);
    let report = compare_scores(&candidate, &reference).expect("comparable");
    assert_eq!(report.f1, 1.0, "graces never enter F1");
    assert_eq!(report.duration_accuracy, 1.0);
    assert_eq!(report.grace_mismatches, 1);
    assert_eq!(report.grace_accuracy, 0.0);
    assert!(!report.exact, "a grace mismatch must break exact");
}

#[test]
fn missing_grace_on_one_side_is_a_mismatch() {
    let reference = with_grace(score(vec![vec![(Some(0), 4), (Some(34), 8)]]), 0, 4, &[22]);
    let candidate = score(vec![vec![(Some(0), 4), (Some(34), 8)]]);
    let report = compare_scores(&candidate, &reference).expect("comparable");
    assert_eq!(report.f1, 1.0);
    assert_eq!(report.candidate_grace_notes, 0);
    assert_eq!(report.reference_grace_notes, 1);
    assert_eq!(report.grace_mismatches, 1);
    assert!(!report.exact);
}

#[test]
fn grace_chain_order_matters() {
    let reference = with_grace(score(vec![vec![(Some(0), 8)]]), 0, 0, &[-2, 3]);
    let candidate = with_grace(score(vec![vec![(Some(0), 8)]]), 0, 0, &[3, -2]);
    let report = compare_scores(&candidate, &reference).expect("comparable");
    assert_eq!(report.grace_mismatches, 1);
    assert!(!report.exact);
}

#[test]
fn grace_chains_compare_as_absolute_pitches_across_base_hints() {
    // Same sounding music, different base_note_hint: offsets differ but the
    // absolute grace pitches agree.
    let reference = with_grace(score(vec![vec![(Some(12), 8)]]), 0, 0, &[10]);
    let mut candidate = with_grace(score(vec![vec![(Some(0), 8)]]), 0, 0, &[-2]);
    candidate.base_note_hint = Some(72);
    let report = compare_scores(&candidate, &reference).expect("comparable");
    assert_eq!(report.matched, 1);
    assert_eq!(report.grace_matches, 1);
    assert!(report.exact);
}

#[test]
fn scores_without_graces_report_neutral_grace_fields() {
    let reference = score(vec![vec![(Some(0), 4), (Some(7), 4)]]);
    let report = compare_scores(&reference, &reference).expect("comparable");
    assert_eq!(report.candidate_grace_notes, 0);
    assert_eq!(report.grace_matches, 0);
    assert_eq!(report.grace_mismatches, 0);
    assert_eq!(report.grace_accuracy, 1.0);
    assert!(report.exact);
}
