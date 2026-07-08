use super::*;

/// Wraps measures for one part in a minimal partwise document.
fn doc(measures: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <part-list>
    <score-part id="P1"><part-name>Test</part-name></score-part>
  </part-list>
  <part id="P1">{}</part>
</score-partwise>"#,
        measures
    )
}

fn note(pitch: &str, octave: i32, alter: Option<i32>, duration: i32, extra: &str) -> String {
    let alter = alter
        .map(|a| format!("<alter>{}</alter>", a))
        .unwrap_or_default();
    format!(
        "<note><pitch><step>{}</step>{}<octave>{}</octave></pitch>\
         <duration>{}</duration><voice>1</voice>{}</note>",
        pitch, alter, octave, duration, extra
    )
}

fn convert(measures: &str) -> (Score, ConvertReport) {
    convert_musicxml(&doc(measures)).expect("conversion should succeed")
}

/// Serialized shape of a cell, for compact assertions: note offsets, `H` for
/// held, `.` for rests.
fn shape(steps: &[Step]) -> String {
    steps
        .iter()
        .map(|step| {
            if step.held {
                "H".to_string()
            } else {
                match step.note {
                    Some(offset) => offset.to_string(),
                    None => ".".to_string(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn maps_pitch_step_alter_octave_to_midi_offsets() {
    // C4 = 60 (offset 0), Eb4 = 63 (offset 3), F#5 = 78 (offset 18).
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>3</beats><beat-type>4</beat-type></time></attributes>
          {}{}{}
        </measure>"#,
        note("C", 4, None, 1, ""),
        note("E", 4, Some(-1), 1, ""),
        note("F", 5, Some(1), 1, "")
    ));
    assert_eq!(score.base_note_hint, Some(60));
    assert_eq!(score.cells.len(), 1);
    assert_eq!(shape(&score.cells[0]), "0 3 18");
    assert_eq!(score.rhythm_grid.as_deref(), Some("quarter_note"));
}

#[test]
fn renders_durations_as_held_steps_and_rests_as_nulls() {
    // Quarter + eighth rest + eighth on an eighth grid.
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>2</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {}<note><rest/><duration>1</duration><voice>1</voice></note>{}
        </measure>"#,
        note("C", 4, None, 2, ""),
        note("D", 4, None, 1, "")
    ));
    assert_eq!(score.rhythm_grid.as_deref(), Some("8th_note"));
    assert_eq!(shape(&score.cells[0]), "0 H . 2");
}

#[test]
fn merges_ties_into_held_continuations() {
    // A C4 quarter tied across the barline continues as a held step, not a
    // second onset.
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {}{}
        </measure>
        <measure number="2">{}{}</measure>"#,
        note("C", 4, None, 1, ""),
        note("C", 4, None, 1, r#"<tie type="start"/>"#),
        note("C", 4, None, 1, r#"<tie type="stop"/>"#),
        note("D", 4, None, 1, "")
    ));
    assert_eq!(report.ties_merged, 1);
    assert_eq!(shape(&score.cells[0]), "0 0 H 2");
}

#[test]
fn unmatched_tie_stop_becomes_a_new_onset_with_warning() {
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {}{}
        </measure>"#,
        note("C", 4, None, 1, ""),
        note("E", 4, None, 1, r#"<tie type="stop"/>"#)
    ));
    assert_eq!(report.ties_merged, 0);
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(shape(&score.cells[0]), "0 4");
}

#[test]
fn splits_chords_into_pitch_ordered_lanes() {
    // A two-note chord: lane 0 takes the higher pitch.
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>1</beats><beat-type>4</beat-type></time></attributes>
          {}<note><chord/><pitch><step>E</step><octave>4</octave></pitch>
            <duration>1</duration><voice>1</voice></note>
        </measure>"#,
        note("C", 4, None, 1, "")
    ));
    assert_eq!(score.cells.len(), 2);
    assert_eq!(shape(&score.cells[0]), "4");
    assert_eq!(shape(&score.cells[1]), "0");
}

#[test]
fn separates_voices_into_cells_via_backup() {
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {}{}
          <backup><duration>2</duration></backup>
          <note><pitch><step>C</step><octave>3</octave></pitch>
            <duration>2</duration><voice>2</voice></note>
        </measure>"#,
        note("E", 4, None, 1, ""),
        note("G", 4, None, 1, "")
    ));
    assert_eq!(score.cells.len(), 2);
    assert_eq!(
        report.lanes,
        vec![("voice 1".to_string(), 1), ("voice 2".to_string(), 1)]
    );
    assert_eq!(shape(&score.cells[0]), "4 7");
    assert_eq!(shape(&score.cells[1]), "-12 H");
}

#[test]
fn infers_the_grid_from_the_finest_subdivision() {
    // A dotted eighth + sixteenth forces a sixteenth grid.
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>4</divisions>
            <time><beats>1</beats><beat-type>4</beat-type></time></attributes>
          {}{}
        </measure>"#,
        note("C", 4, None, 3, ""),
        note("D", 4, None, 1, "")
    ));
    assert_eq!(score.rhythm_grid.as_deref(), Some("16th_note"));
    assert_eq!(shape(&score.cells[0]), "0 H H 2");
}

#[test]
fn all_cells_share_the_measure_grid_length() {
    // Voice 2 appears only in measure 2; both cells still span both measures.
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {}{}
        </measure>
        <measure number="2">
          {}{}
          <backup><duration>2</duration></backup>
          <note><pitch><step>C</step><octave>3</octave></pitch>
            <duration>2</duration><voice>2</voice></note>
        </measure>"#,
        note("C", 4, None, 1, ""),
        note("D", 4, None, 1, ""),
        note("E", 4, None, 1, ""),
        note("F", 4, None, 1, "")
    ));
    assert_eq!(report.measures, 2);
    assert_eq!(report.steps_per_cell, 4);
    for cell in &score.cells {
        assert_eq!(cell.len(), 4);
    }
    assert_eq!(shape(&score.cells[1]), ". . -12 H");
}

#[test]
fn skips_grace_notes_without_consuming_grid_time() {
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          <note><grace/><pitch><step>B</step><octave>4</octave></pitch>
            <voice>1</voice></note>
          {}{}
        </measure>"#,
        note("C", 4, None, 1, ""),
        note("D", 4, None, 1, "")
    ));
    assert_eq!(report.grace_notes_skipped, 1);
    assert_eq!(shape(&score.cells[0]), "0 2");
}

#[test]
fn extracts_metadata_preferring_engraved_credits() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <work><work-title>Untitled score</work-title></work>
  <identification>
    <creator type="composer">Composer / arranger</creator>
  </identification>
  <credit page="1"><credit-type>title</credit-type>
    <credit-words>The Flow of Water</credit-words></credit>
  <credit page="1"><credit-type>composer</credit-type>
    <credit-words>Grant Damron</credit-words></credit>
  <part-list><score-part id="P1"><part-name>Piano</part-name></score-part></part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>1</divisions>
        <key><fifths>-4</fifths></key>
        <time><beats>4</beats><beat-type>4</beat-type></time></attributes>
      <direction><sound tempo="60"/></direction>
      <note><pitch><step>C</step><octave>4</octave></pitch>
        <duration>4</duration><voice>1</voice></note>
    </measure>
  </part>
</score-partwise>"#;
    let (score, _) = convert_musicxml(xml).expect("conversion should succeed");
    assert_eq!(score.schema.as_deref(), Some(SCORE_SCHEMA_V1));
    assert_eq!(score.title.as_deref(), Some("The Flow of Water"));
    assert_eq!(score.composer.as_deref(), Some("Grant Damron"));
    assert_eq!(score.key.as_deref(), Some("Ab major"));
    assert_eq!(score.tempo, Some(60.0));
    let time_signature = score.time_signature.expect("time signature");
    assert_eq!(time_signature.beats_per_measure, 4);
    assert_eq!(time_signature.beat_unit, 4);
}

#[test]
fn maps_minor_mode_key_names() {
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <key><fifths>-4</fifths><mode>minor</mode></key>
            <time><beats>1</beats><beat-type>4</beat-type></time></attributes>
          {}
        </measure>"#,
        note("F", 4, None, 1, "")
    ));
    assert_eq!(score.key.as_deref(), Some("F minor"));
}

#[test]
fn tempo_changes_compile_into_a_tempo_map() {
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          <direction><sound tempo="60"/></direction>
          {}
          <direction><sound tempo="124"/></direction>
          {}
        </measure>"#,
        note("C", 4, None, 1, ""),
        note("D", 4, None, 1, "")
    ));
    // Notes sit on quarter-note steps 0 and 1; the tempo marks land at the
    // same positions, giving a two-entry map with the initial tempo first.
    assert_eq!(score.tempo, Some(60.0));
    assert_eq!(
        score.tempo_map,
        vec![
            TempoPoint { at_step: 0, bpm: 60.0 },
            TempoPoint { at_step: 1, bpm: 124.0 },
        ]
    );
}

#[test]
fn constant_tempo_emits_no_map() {
    // A restated identical tempo is not a change: no map, just the scalar.
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          <direction><sound tempo="90"/></direction>
          {}
          <direction><sound tempo="90"/></direction>
          {}
        </measure>"#,
        note("C", 4, None, 1, ""),
        note("D", 4, None, 1, "")
    ));
    assert_eq!(score.tempo, Some(90.0));
    assert!(score.tempo_map.is_empty());
}

#[test]
fn meter_changes_resize_measures_and_warn() {
    // 4/4 then 3/4: the grid keeps exact measure lengths (4 + 3 quarters)
    // while the metadata records only the initial signature.
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>4</beats><beat-type>4</beat-type></time></attributes>
          {}
        </measure>
        <measure number="2">
          <attributes><time><beats>3</beats><beat-type>4</beat-type></time></attributes>
          {}
        </measure>"#,
        note("C", 4, None, 4, ""),
        note("D", 4, None, 3, "")
    ));
    assert_eq!(report.steps_per_cell, 7);
    assert_eq!(shape(&score.cells[0]), "0 H H H 2 H H");
    let time_signature = score.time_signature.expect("time signature");
    assert_eq!(time_signature.beats_per_measure, 4);
    assert!(report
        .warnings
        .iter()
        .any(|w| w.contains("time signature changes to 3/4 at measure 2")));
}

#[test]
fn implicit_pickup_measures_use_their_content_length() {
    // A one-beat pickup before a 4/4 measure: 1 + 4 quarters on the grid.
    let (score, report) = convert(&format!(
        r#"<measure number="0" implicit="yes">
          <attributes><divisions>1</divisions>
            <time><beats>4</beats><beat-type>4</beat-type></time></attributes>
          {}
        </measure>
        <measure number="1">{}</measure>"#,
        note("G", 3, None, 1, ""),
        note("C", 4, None, 4, "")
    ));
    assert_eq!(report.steps_per_cell, 5);
    assert_eq!(shape(&score.cells[0]), "-5 0 H H H");
}

#[test]
fn rejects_overfull_measures() {
    let err = convert_musicxml(&doc(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>1</beats><beat-type>4</beat-type></time></attributes>
          {}{}
        </measure>"#,
        note("C", 4, None, 1, ""),
        note("D", 4, None, 1, "")
    )))
    .expect_err("overfull measure must be rejected");
    assert!(
        err.contains("longer than the 1/4 time signature"),
        "{}",
        err
    );
}

#[test]
fn rejects_timewise_documents() {
    let err =
        convert_musicxml(r#"<?xml version="1.0"?><score-timewise version="4.0"></score-timewise>"#)
            .expect_err("timewise must be rejected");
    assert!(err.contains("score-timewise"), "{}", err);
}

#[test]
fn output_validates_and_is_deterministic() {
    let measures = format!(
        r#"<measure number="1">
          <attributes><divisions>2</divisions>
            <key><fifths>0</fifths></key>
            <time><beats>4</beats><beat-type>4</beat-type></time></attributes>
          {}{}{}
          <note><rest/><duration>2</duration><voice>1</voice></note>
        </measure>"#,
        note("C", 4, None, 2, ""),
        note("E", 4, None, 2, ""),
        note("G", 4, None, 2, "")
    );
    let (first, _) = convert(&measures);
    let (second, _) = convert(&measures);
    let first_json = first.to_json().expect("serializes");
    let second_json = second.to_json().expect("serializes");
    assert_eq!(first_json, second_json, "conversion must be deterministic");

    let value = serde_json::to_value(&first).expect("to_value");
    crate::invention::score::validate_score(&value).expect("output must validate");
}
