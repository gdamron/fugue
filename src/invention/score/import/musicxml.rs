//! MusicXML → `fugue.score.v1` conversion.
//!
//! This is the deterministic reference path for score ingest: it reads a
//! `score-partwise` MusicXML document and produces the same asset shape the
//! agent PDF-import skill emits, so its output can serve as ground truth for
//! the import-accuracy harness.
//!
//! Mapping model:
//! - Time is tracked as exact rationals in quarter notes; the rhythm grid is
//!   the GCD of every onset, duration, and measure length, so quantization is
//!   lossless by construction (inputs that don't share a grid keep an exact,
//!   finer one).
//! - Each MusicXML voice becomes one or more cells ("lanes"): chords and
//!   overlapping sustains within a voice are split across lanes by a greedy
//!   interval coloring (higher pitches claim lower lanes first).
//! - Ties merge into a single note event and render as `{ "held": true }`
//!   continuation steps; rests are `{ "note": null }` steps.
//! - Note offsets are relative to a fixed `base_note_hint` of middle C (60).

mod metadata;

use std::collections::BTreeMap;

use crate::invention::score::{Score, TempoPoint, SCORE_SCHEMA_V1};
use crate::invention::TimeSignature;
use crate::modules::Step;

use crate::music::{note_value_name, Note, Rat};
use metadata::extract_metadata;

/// Base MIDI note all step offsets are relative to. Middle C keeps every
/// valid MIDI pitch (0..=127) within the sequencers' `i8` offset range.
pub const BASE_NOTE: i64 = 60;

/// Non-fatal observations gathered during conversion, for the CLI report.
#[derive(Debug, Default)]
pub struct ConvertReport {
    /// Measures converted (from the first part).
    pub measures: usize,
    /// Steps in every cell.
    pub steps_per_cell: usize,
    /// Human-readable rhythm grid name written to the score.
    pub rhythm_grid: String,
    /// Cells per `(part id, voice)` in output order, as `(label, lanes)`.
    pub lanes: Vec<(String, usize)>,
    /// Tie stop events merged into a preceding note.
    pub ties_merged: usize,
    /// Grace/cue notes skipped (they occupy no grid time).
    pub grace_notes_skipped: usize,
    /// Warnings to surface to the user.
    pub warnings: Vec<String>,
}

/// Converts a MusicXML string into a validated-shape [`Score`].
pub fn convert_musicxml(xml: &str) -> Result<(Score, ConvertReport), String> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(xml, options)
        .map_err(|err| format!("invalid MusicXML: {}", err))?;
    let root = doc.root_element();
    if root.has_tag_name("score-timewise") {
        return Err("score-timewise MusicXML is not supported; export partwise".to_string());
    }
    if !root.has_tag_name("score-partwise") {
        return Err(format!(
            "expected a <score-partwise> MusicXML document, found <{}>",
            root.tag_name().name()
        ));
    }

    let mut report = ConvertReport::default();
    let mut parts = Vec::new();
    for part in root.children().filter(|n| n.has_tag_name("part")) {
        let id = part.attribute("id").unwrap_or("?").to_string();
        parts.push(convert_part(part, id, &mut report)?);
    }
    let first = parts.first().ok_or("MusicXML document has no parts")?;
    if first.capacities.is_empty() {
        return Err("MusicXML document has no measures".to_string());
    }
    for other in &parts[1..] {
        if other.capacities != first.capacities {
            return Err(format!(
                "parts '{}' and '{}' disagree on measure lengths",
                first.id, other.id
            ));
        }
    }
    report.measures = first.capacities.len();

    // The grid is the GCD of every onset, duration, and measure length, so
    // every event lands on an integer step index.
    let mut grid: Option<Rat> = None;
    let mut fold = |value: Rat| {
        if !value.is_zero() {
            grid = Some(match grid {
                Some(current) => current.gcd(value),
                None => value,
            });
        }
    };
    for capacity in &first.capacities {
        fold(*capacity);
    }
    for part in &parts {
        for events in part.voices.values() {
            for event in events {
                fold(event.onset);
                fold(event.duration);
            }
        }
    }
    // Fold tempo-mark onsets in too, so every mark lands on an integer step
    // (metric modulations sit on bar/beat boundaries, so in practice this
    // never refines the grid; it only guarantees exactness).
    for (onset, _) in &first.tempo_events {
        fold(*onset);
    }
    let grid = grid.ok_or("MusicXML document has no notes")?;
    let total: Rat = first
        .capacities
        .iter()
        .fold(Rat::new(0, 1), |sum, c| sum + *c);
    let steps_per_cell = total
        .div_exact(grid)
        .ok_or("internal: grid does not divide the piece length")?;
    report.steps_per_cell = steps_per_cell as usize;
    report.rhythm_grid = note_value_name(grid);

    // Merge ties, split voices into lanes, and emit step cells.
    let mut cells: Vec<Vec<Step>> = Vec::new();
    let multi_part = parts.len() > 1;
    for part in &parts {
        for (key, events) in &part.voices {
            let merged = merge_ties(events.clone(), &mut report);
            let lanes = assign_lanes(merged);
            let label = if multi_part {
                format!("part {} voice {}", part.id, key.1)
            } else {
                format!("voice {}", key.1)
            };
            report.lanes.push((label.clone(), lanes.len()));
            for lane in lanes {
                cells.push(emit_lane(&lane, grid, steps_per_cell as usize, &label)?);
            }
        }
    }

    // Compile positioned tempo marks (first part is authoritative, matching
    // measure lengths) into an ordered tempo map on the step grid.
    let (tempo, tempo_map) = build_tempo_map(&first.tempo_events, grid)?;

    let metadata = extract_metadata(root);
    let score = Score {
        schema: Some(SCORE_SCHEMA_V1.to_string()),
        title: metadata.title,
        composer: metadata.composer,
        key: metadata.key,
        tempo,
        tempo_map,
        time_signature: metadata.time_signature,
        base_note_hint: Some(BASE_NOTE),
        rhythm_grid: Some(report.rhythm_grid.clone()),
        cells,
    };
    Ok((score, report))
}

/// Turns positioned tempo marks into `(initial tempo, tempo_map)`.
///
/// Marks are placed on the step grid, ordered, and de-duplicated: a later mark
/// at the same step supersedes an earlier one, and a mark that restates the
/// preceding tempo is dropped. The map is emitted only when the piece actually
/// changes tempo (two or more distinct tempos); a constant-tempo piece yields
/// just the scalar `tempo`, so existing single-tempo imports are unchanged.
fn build_tempo_map(
    tempo_events: &[(Rat, f32)],
    grid: Rat,
) -> Result<(Option<f32>, Vec<TempoPoint>), String> {
    let mut placed: Vec<(u64, f32)> = Vec::with_capacity(tempo_events.len());
    for (onset, bpm) in tempo_events {
        let at_step = onset
            .div_exact(grid)
            .ok_or("internal: tempo mark is not on the step grid")? as u64;
        placed.push((at_step, *bpm));
    }
    // Stable sort keeps document order among marks sharing a step.
    placed.sort_by_key(|(at_step, _)| *at_step);

    let mut map: Vec<TempoPoint> = Vec::new();
    for (at_step, bpm) in placed {
        if let Some(last) = map.last_mut() {
            if last.at_step == at_step {
                last.bpm = bpm;
                continue;
            }
            if last.bpm == bpm {
                continue;
            }
        }
        map.push(TempoPoint { at_step, bpm });
    }

    let tempo = map.first().map(|point| point.bpm);
    // A single (or collapsed-to-one) tempo needs no map.
    let tempo_map = if map.len() > 1 { map } else { Vec::new() };
    Ok((tempo, tempo_map))
}

/// A sounding note with an exact global onset and duration (quarter notes).
#[derive(Debug, Clone)]
struct NoteEvent {
    onset: Rat,
    duration: Rat,
    midi: i64,
    tie_start: bool,
    tie_stop: bool,
}

/// One `<part>` reduced to per-voice note events plus measure lengths.
struct PartEvents {
    id: String,
    /// Keyed by `(numeric voice sort key, voice label)` for stable ordering.
    voices: BTreeMap<(u32, String), Vec<NoteEvent>>,
    capacities: Vec<Rat>,
    /// Playback tempo marks as `(global onset in quarter notes, quarter-note
    /// BPM)`, in document order. Sourced from `<sound tempo>` elements, which
    /// is how notation editors encode tempo and metric modulations.
    tempo_events: Vec<(Rat, f32)>,
}

/// Reads a quarter-note BPM from a `<sound tempo>` element or a `<direction>`
/// that contains one. `<sound tempo>` is always quarter notes per minute in
/// MusicXML, regardless of the displayed metronome unit, so metric
/// modulations resolve to their true playback tempo.
fn tempo_from(element: roxmltree::Node) -> Option<f32> {
    let sound = if element.has_tag_name("sound") {
        Some(element)
    } else {
        element.descendants().find(|n| n.has_tag_name("sound"))
    };
    sound
        .and_then(|n| n.attribute("tempo"))
        .and_then(|t| t.parse::<f32>().ok())
        .filter(|bpm| bpm.is_finite() && *bpm > 0.0)
}

fn convert_part(
    part: roxmltree::Node,
    id: String,
    report: &mut ConvertReport,
) -> Result<PartEvents, String> {
    let mut voices: BTreeMap<(u32, String), Vec<NoteEvent>> = BTreeMap::new();
    let mut capacities = Vec::new();
    let mut tempo_events: Vec<(Rat, f32)> = Vec::new();
    let mut divisions: i64 = 1;
    let mut time_signature = TimeSignature::default();
    let mut seen_time = false;
    let mut measure_start = Rat::new(0, 1);

    for measure in part.children().filter(|n| n.has_tag_name("measure")) {
        let number = measure.attribute("number").unwrap_or("?");
        let mut cursor = Rat::new(0, 1);
        let mut high_water = cursor;
        // Onset of the most recent non-chord note, for `<chord/>` members.
        let mut chord_onset: Option<Rat> = None;

        for element in measure.children().filter(|n| n.is_element()) {
            match element.tag_name().name() {
                "attributes" => {
                    if let Some(text) = child_text(element, "divisions") {
                        divisions = text.parse::<i64>().map_err(|_| {
                            format!("measure {}: invalid <divisions> '{}'", number, text)
                        })?;
                        if divisions <= 0 {
                            return Err(format!(
                                "measure {}: <divisions> must be positive",
                                number
                            ));
                        }
                    }
                    if let Some(time) = element.children().find(|n| n.has_tag_name("time")) {
                        if let (Some(beats), Some(unit)) =
                            (child_text(time, "beats"), child_text(time, "beat-type"))
                        {
                            if let (Ok(beats), Ok(unit)) =
                                (beats.parse::<u32>(), unit.parse::<u32>())
                            {
                                if beats > 0 && unit > 0 {
                                    let next = TimeSignature {
                                        beats_per_measure: beats,
                                        beat_unit: unit,
                                    };
                                    if seen_time && next != time_signature {
                                        report.warnings.push(format!(
                                            "time signature changes to {}/{} at measure {}; \
                                             fugue.score.v1 metadata records only the initial \
                                             one (measure lengths on the grid stay exact)",
                                            beats, unit, number
                                        ));
                                    }
                                    seen_time = true;
                                    time_signature = next;
                                }
                            }
                        }
                    }
                }
                "backup" | "forward" => {
                    let duration = required_duration(element, number, divisions)?;
                    cursor = if element.has_tag_name("backup") {
                        cursor - duration
                    } else {
                        cursor + duration
                    };
                    if cursor.is_negative() {
                        return Err(format!(
                            "measure {}: <backup> moved the cursor before the barline",
                            number
                        ));
                    }
                    high_water = high_water.max(cursor);
                }
                "note" => {
                    if element.children().any(|n| n.has_tag_name("grace"))
                        || element.children().any(|n| n.has_tag_name("cue"))
                    {
                        report.grace_notes_skipped += 1;
                        continue;
                    }
                    let duration = required_duration(element, number, divisions)?;
                    let is_chord = element.children().any(|n| n.has_tag_name("chord"));
                    let onset = if is_chord {
                        chord_onset.ok_or_else(|| {
                            format!("measure {}: <chord/> note without a preceding note", number)
                        })?
                    } else {
                        cursor
                    };

                    if let Some(midi) = note_midi(element, number)? {
                        let voice = element
                            .children()
                            .find(|n| n.has_tag_name("voice"))
                            .and_then(|n| n.text())
                            .unwrap_or("1")
                            .trim()
                            .to_string();
                        let sort = voice.parse::<u32>().unwrap_or(u32::MAX);
                        let (tie_start, tie_stop) = tie_flags(element);
                        voices.entry((sort, voice)).or_default().push(NoteEvent {
                            onset: measure_start + onset,
                            duration,
                            midi,
                            tie_start,
                            tie_stop,
                        });
                    }

                    if !is_chord {
                        chord_onset = Some(cursor);
                        cursor = cursor + duration;
                        high_water = high_water.max(cursor);
                    }
                }
                "direction" | "sound" => {
                    // Tempo marks apply at the current time position.
                    if let Some(bpm) = tempo_from(element) {
                        tempo_events.push((measure_start + cursor, bpm));
                    }
                }
                _ => {}
            }
        }

        // Nominal measure length comes from the time signature; pickup
        // (implicit) measures are as long as their content.
        let nominal = Rat::new(
            i64::from(time_signature.beats_per_measure) * 4,
            i64::from(time_signature.beat_unit),
        );
        let capacity = if measure.attribute("implicit") == Some("yes") {
            high_water
        } else {
            nominal
        };
        if high_water > capacity {
            return Err(format!(
                "measure {}: content is longer than the {}/{} time signature allows",
                number, time_signature.beats_per_measure, time_signature.beat_unit
            ));
        }
        if !capacity.is_positive() {
            return Err(format!("measure {}: empty implicit measure", number));
        }
        measure_start = measure_start + capacity;
        capacities.push(capacity);
    }

    Ok(PartEvents {
        id,
        voices,
        capacities,
        tempo_events,
    })
}

fn child_text<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|n| n.has_tag_name(name))
        .and_then(|n| n.text())
        .map(str::trim)
}

fn required_duration(
    element: roxmltree::Node,
    measure: &str,
    divisions: i64,
) -> Result<Rat, String> {
    let text = child_text(element, "duration").ok_or_else(|| {
        format!(
            "measure {}: <{}> is missing <duration>",
            measure,
            element.tag_name().name()
        )
    })?;
    let value = text
        .parse::<i64>()
        .map_err(|_| format!("measure {}: invalid <duration> '{}'", measure, text))?;
    if value < 0 {
        return Err(format!("measure {}: negative <duration>", measure));
    }
    Ok(Rat::new(value, divisions))
}

/// MIDI note number for a pitched note; `None` for rests and unpitched notes.
///
/// The XML extraction and error context live here; the spelled-pitch → MIDI
/// mapping itself is [`Note::from_spelling`] in `crate::music`.
fn note_midi(element: roxmltree::Node, measure: &str) -> Result<Option<i64>, String> {
    if element.children().any(|n| n.has_tag_name("rest")) {
        return Ok(None);
    }
    let Some(pitch) = element.children().find(|n| n.has_tag_name("pitch")) else {
        // `<unpitched>` percussion and similar carry no pitch to transcribe.
        return Ok(None);
    };
    let step = child_text(pitch, "step")
        .ok_or_else(|| format!("measure {}: <pitch> missing <step>", measure))?;
    let step = match step.chars().next() {
        Some(letter) if step.len() == 1 => letter,
        _ => return Err(format!("measure {}: invalid <step> '{}'", measure, step)),
    };
    let alter = child_text(pitch, "alter")
        .map(|t| {
            t.parse::<f32>()
                .map_err(|_| format!("measure {}: invalid <alter> '{}'", measure, t))
        })
        .transpose()?
        .unwrap_or(0.0);
    if alter.fract() != 0.0 {
        return Err(format!(
            "measure {}: microtonal <alter> {} is not representable",
            measure, alter
        ));
    }
    let octave = child_text(pitch, "octave")
        .ok_or_else(|| format!("measure {}: <pitch> missing <octave>", measure))?
        .parse::<i32>()
        .map_err(|_| format!("measure {}: invalid <octave>", measure))?;
    let note = Note::from_spelling(step, alter as i32, octave).ok_or_else(|| {
        format!(
            "measure {}: '{}' (alter {}, octave {}) is not a MIDI pitch",
            measure, step, alter, octave
        )
    })?;
    Ok(Some(i64::from(note.midi_note)))
}

fn tie_flags(element: roxmltree::Node) -> (bool, bool) {
    let mut start = false;
    let mut stop = false;
    for tie in element.children().filter(|n| n.has_tag_name("tie")) {
        match tie.attribute("type") {
            Some("start") => start = true,
            Some("stop") => stop = true,
            _ => {}
        }
    }
    (start, stop)
}

/// Merges tie chains into single long events. A tie-stop event joins the open
/// tie-start event of the same pitch that ends exactly where it begins.
fn merge_ties(mut events: Vec<NoteEvent>, report: &mut ConvertReport) -> Vec<NoteEvent> {
    events.sort_by(|a, b| a.onset.cmp(&b.onset).then(b.midi.cmp(&a.midi)));
    let mut merged: Vec<NoteEvent> = Vec::with_capacity(events.len());
    // Open tie chains awaiting continuation, by pitch.
    let mut open: BTreeMap<i64, usize> = BTreeMap::new();
    for event in events {
        if event.tie_stop {
            if let Some(&idx) = open.get(&event.midi) {
                let end = merged[idx].onset + merged[idx].duration;
                if end == event.onset {
                    merged[idx].duration = merged[idx].duration + event.duration;
                    report.ties_merged += 1;
                    if !event.tie_start {
                        open.remove(&event.midi);
                    }
                    continue;
                }
            }
            report.warnings.push(format!(
                "tie into pitch {} could not be matched to a preceding note; kept as a new onset",
                event.midi
            ));
        }
        let idx = merged.len();
        merged.push(event.clone());
        if event.tie_start {
            open.insert(event.midi, idx);
        }
    }
    merged
}

/// Splits a voice's events into monophonic lanes by greedy interval coloring:
/// events are taken in onset order (higher pitch first at equal onsets) and
/// claim the lowest-numbered lane that is free.
fn assign_lanes(mut events: Vec<NoteEvent>) -> Vec<Vec<NoteEvent>> {
    events.sort_by(|a, b| a.onset.cmp(&b.onset).then(b.midi.cmp(&a.midi)));
    let mut lanes: Vec<Vec<NoteEvent>> = Vec::new();
    let mut lane_ends: Vec<Rat> = Vec::new();
    for event in events {
        let end = event.onset + event.duration;
        match lane_ends.iter().position(|&e| e <= event.onset) {
            Some(lane) => {
                lane_ends[lane] = end;
                lanes[lane].push(event);
            }
            None => {
                lane_ends.push(end);
                lanes.push(vec![event]);
            }
        }
    }
    lanes
}

/// Renders one lane's events onto the step grid.
fn emit_lane(
    events: &[NoteEvent],
    grid: Rat,
    steps_per_cell: usize,
    label: &str,
) -> Result<Vec<Step>, String> {
    let mut steps = vec![Step::rest(); steps_per_cell];
    for event in events {
        let start = event
            .onset
            .div_exact(grid)
            .ok_or_else(|| format!("internal: {} onset off the rhythm grid", label))?
            as usize;
        let len = event
            .duration
            .div_exact(grid)
            .ok_or_else(|| format!("internal: {} duration off the rhythm grid", label))?
            as usize;
        if len == 0 || start + len > steps_per_cell {
            return Err(format!(
                "internal: {} event exceeds the piece length",
                label
            ));
        }
        steps[start] = Step::note((event.midi - BASE_NOTE) as i8);
        for step in steps.iter_mut().skip(start + 1).take(len - 1) {
            *step = Step::held();
        }
    }
    Ok(steps)
}

#[cfg(test)]
mod tests;
