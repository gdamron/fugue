//! Note-level comparison of two `fugue.score.v1` documents.
//!
//! The accuracy metric for score ingest: a candidate transcription (e.g. an
//! agent's PDF import) is scored against a reference (e.g. the MusicXML
//! converter's ground truth). Both scores are flattened to absolute-pitch
//! note events on an exact time base, so the comparison is robust to
//! different cell/lane decompositions and different rhythm grids — only
//! *what sounds when* matters.
//!
//! Matching is exact on `(onset, pitch)`; among matched notes, durations are
//! compared separately. Unmatched notes are additionally classified into two
//! diagnostic buckets (octave errors, near-miss timing) to make transcription
//! failure modes legible in reports.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::invention::score::Score;
use crate::music::{note_value_from_name, Rat};

/// Default base note when a score omits `base_note_hint` (middle C, matching
/// the sequencers' convention).
const DEFAULT_BASE_NOTE: i64 = 60;

/// Result of comparing a candidate score against a reference.
#[derive(Debug, Clone, Serialize)]
pub struct CompareReport {
    /// Notes present in both scores at the same onset and pitch.
    pub matched: usize,
    /// Candidate notes with no reference counterpart (false positives).
    pub candidate_only: usize,
    /// Reference notes the candidate missed (false negatives).
    pub reference_only: usize,
    /// matched / candidate notes (1.0 when the candidate is empty).
    pub precision: f64,
    /// matched / reference notes (1.0 when the reference is empty).
    pub recall: f64,
    /// Harmonic mean of precision and recall.
    pub f1: f64,
    /// Matched notes whose durations are also exactly equal.
    pub duration_matches: usize,
    /// duration_matches / matched (1.0 when nothing matched).
    pub duration_accuracy: f64,
    /// Unmatched pairs at the same onset whose pitches differ by octaves.
    pub octave_errors: usize,
    /// Unmatched pairs with the same pitch within one grid step.
    pub timing_near_misses: usize,
    /// Total sounding length of each score, in quarter notes, as a string
    /// fraction (e.g. "151" or "603/4").
    pub candidate_duration_quarters: String,
    pub reference_duration_quarters: String,
    /// Whether the two scores span the same total duration.
    pub total_duration_match: bool,
    /// Measure counts, when a score's time signature divides its length
    /// evenly (meter changes make this undefined from the asset alone).
    pub candidate_measures: Option<u64>,
    pub reference_measures: Option<u64>,
    /// Perfect transcription: F1 = 1, all durations equal, same total length.
    pub exact: bool,
}

/// A note event on the exact time base: onset and duration in quarter notes.
#[derive(Debug, Clone, Copy)]
struct NoteEvent {
    onset: Rat,
    duration: Rat,
    pitch: i64,
}

/// Compares `candidate` against `reference` at the note level.
///
/// Fails only when a score carries an unparseable `rhythm_grid` (a missing
/// grid falls back to one quarter note per step, which is only meaningful if
/// both sides agree — importers always write the grid).
pub fn compare_scores(candidate: &Score, reference: &Score) -> Result<CompareReport, String> {
    let candidate_grid = score_grid(candidate, "candidate")?;
    let reference_grid = score_grid(reference, "reference")?;
    let candidate_events = flatten(candidate, candidate_grid);
    let reference_events = flatten(reference, reference_grid);

    // Index the reference by (onset, pitch); values are the durations of
    // every reference note at that position (chords across cells can double
    // a pitch, so this is a multiset).
    let mut unmatched_reference: BTreeMap<(Rat, i64), Vec<Rat>> = BTreeMap::new();
    for event in &reference_events {
        unmatched_reference
            .entry((event.onset, event.pitch))
            .or_default()
            .push(event.duration);
    }

    let mut matched = 0usize;
    let mut duration_matches = 0usize;
    let mut candidate_leftover: Vec<NoteEvent> = Vec::new();
    for event in &candidate_events {
        let Some(durations) = unmatched_reference.get_mut(&(event.onset, event.pitch)) else {
            candidate_leftover.push(*event);
            continue;
        };
        if durations.is_empty() {
            candidate_leftover.push(*event);
            continue;
        }
        matched += 1;
        // Prefer a duration-exact pairing when one exists.
        if let Some(index) = durations.iter().position(|d| *d == event.duration) {
            durations.swap_remove(index);
            duration_matches += 1;
        } else {
            durations.pop();
        }
    }
    let reference_leftover: Vec<(Rat, i64)> = unmatched_reference
        .iter()
        .flat_map(|(&key, durations)| durations.iter().map(move |_| key))
        .collect();

    // Diagnostics: classify unmatched pairs without consuming them twice.
    let mut octave_errors = 0usize;
    let mut timing_near_misses = 0usize;
    let step = candidate_grid.max(reference_grid);
    let mut used = vec![false; reference_leftover.len()];
    for event in &candidate_leftover {
        if let Some(i) = reference_leftover.iter().enumerate().position(|(i, r)| {
            !used[i] && r.0 == event.onset && r.1 != event.pitch && (r.1 - event.pitch) % 12 == 0
        }) {
            used[i] = true;
            octave_errors += 1;
            continue;
        }
        if let Some(i) = reference_leftover.iter().enumerate().position(|(i, r)| {
            !used[i]
                && r.1 == event.pitch
                && r.0 != event.onset
                && within_one_step(r.0, event.onset, step)
        }) {
            used[i] = true;
            timing_near_misses += 1;
        }
    }

    let candidate_total = total_duration(candidate, candidate_grid);
    let reference_total = total_duration(reference, reference_grid);
    let candidate_count = candidate_events.len();
    let reference_count = reference_events.len();
    let precision = ratio(matched, candidate_count);
    let recall = ratio(matched, reference_count);
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };
    let duration_accuracy = ratio(duration_matches, matched);
    let total_duration_match = candidate_total == reference_total;
    let exact = matched == candidate_count
        && matched == reference_count
        && duration_matches == matched
        && total_duration_match;

    Ok(CompareReport {
        matched,
        candidate_only: candidate_count - matched,
        reference_only: reference_count - matched,
        precision,
        recall,
        f1,
        duration_matches,
        duration_accuracy,
        octave_errors,
        timing_near_misses,
        candidate_duration_quarters: format_quarters(candidate_total),
        reference_duration_quarters: format_quarters(reference_total),
        total_duration_match,
        candidate_measures: measure_count(candidate, candidate_total),
        reference_measures: measure_count(reference, reference_total),
        exact,
    })
}

fn score_grid(score: &Score, label: &str) -> Result<Rat, String> {
    match &score.rhythm_grid {
        Some(name) => note_value_from_name(name)
            .ok_or_else(|| format!("{} has an unrecognized rhythm_grid '{}'", label, name)),
        None => Ok(Rat::new(1, 1)),
    }
}

/// Flattens every cell to absolute-pitch note events on the exact time base.
fn flatten(score: &Score, grid: Rat) -> Vec<NoteEvent> {
    let base = score.base_note_hint.unwrap_or(DEFAULT_BASE_NOTE);
    let mut events = Vec::new();
    for cell in &score.cells {
        let mut current: Option<(usize, i64)> = None;
        let mut end = 0usize;
        for (index, step) in cell.iter().enumerate() {
            if step.held && current.is_some() {
                end = index + 1;
                continue;
            }
            if let Some((start, pitch)) = current.take() {
                events.push(event_at(start, end, pitch, grid));
            }
            if let Some(offset) = step.note {
                current = Some((index, base + i64::from(offset)));
                end = index + 1;
            }
        }
        if let Some((start, pitch)) = current.take() {
            events.push(event_at(start, end, pitch, grid));
        }
    }
    events
}

fn event_at(start: usize, end: usize, pitch: i64, grid: Rat) -> NoteEvent {
    NoteEvent {
        onset: Rat::new(start as i64 * grid.num(), grid.den()),
        duration: Rat::new((end - start) as i64 * grid.num(), grid.den()),
        pitch,
    }
}

/// Total sounding span: the longest cell, in quarter notes.
fn total_duration(score: &Score, grid: Rat) -> Rat {
    let steps = score.cells.iter().map(Vec::len).max().unwrap_or(0);
    Rat::new(steps as i64 * grid.num(), grid.den())
}

fn measure_count(score: &Score, total: Rat) -> Option<u64> {
    let signature = score.time_signature?;
    let measure = Rat::new(
        i64::from(signature.beats_per_measure) * 4,
        i64::from(signature.beat_unit),
    );
    total.div_exact(measure).map(|n| n as u64)
}

fn within_one_step(a: Rat, b: Rat, step: Rat) -> bool {
    let (low, high) = if a <= b { (a, b) } else { (b, a) };
    high - low <= step
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        1.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn format_quarters(value: Rat) -> String {
    if value.den() == 1 {
        value.num().to_string()
    } else {
        format!("{}/{}", value.num(), value.den())
    }
}

#[cfg(test)]
mod tests;
