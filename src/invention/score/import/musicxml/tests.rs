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
            TempoPoint { at_step: 0, bpm: 60.0, ramp: None },
            TempoPoint { at_step: 1, bpm: 124.0, ramp: None },
        ]
    );
}

#[test]
fn warns_on_gradual_tempo_text_direction() {
    // A rit./accel. text direction has no target tempo, so it can't be encoded
    // faithfully — the import warns rather than guessing.
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          <direction><sound tempo="60"/></direction>
          {}
          <direction placement="below">
            <direction-type><words>rit.</words></direction-type>
            <direction-type><dashes type="start" number="1"/></direction-type>
          </direction>
          {}
        </measure>"#,
        note("C", 4, None, 1, ""),
        note("D", 4, None, 1, "")
    ));
    // The single tempo mark yields no map (constant), and the rit. warns.
    assert_eq!(score.tempo, Some(60.0));
    assert!(score.tempo_map.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("gradual tempo change") && w.contains("rit")),
        "expected a rit. warning, got {:?}",
        report.warnings
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

/// Amplitudes of a cell's steps: `Some` only on note onsets that carry one.
fn amplitudes(steps: &[Step]) -> Vec<Option<f32>> {
    steps.iter().map(|step| step.amplitude).collect()
}

fn dynamic(mark: &str) -> String {
    format!(
        "<direction><direction-type><dynamics><{}/></dynamics></direction-type></direction>",
        mark
    )
}

fn wedge(kind: &str) -> String {
    format!(
        r#"<direction><direction-type><wedge type="{}"/></direction-type></direction>"#,
        kind
    )
}

#[test]
fn dynamic_marks_set_step_amplitude_at_onsets() {
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {}{}{}{}
        </measure>"#,
        dynamic("p"),
        note("C", 4, None, 1, ""),
        dynamic("f"),
        note("D", 4, None, 1, "")
    ));
    assert_eq!(report.dynamic_marks, 2);
    assert_eq!(
        amplitudes(&score.cells[0]),
        vec![Some(49.0 / 127.0), Some(96.0 / 127.0)]
    );
}

#[test]
fn notes_before_the_first_mark_carry_no_amplitude() {
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {}{}{}
        </measure>"#,
        note("C", 4, None, 1, ""),
        dynamic("mf"),
        note("D", 4, None, 1, "")
    ));
    assert_eq!(
        amplitudes(&score.cells[0]),
        vec![None, Some(80.0 / 127.0)]
    );
}

#[test]
fn hairpin_interpolates_to_the_next_mark_across_its_span() {
    // p, cresc across two quarters, f: the midpoint onset sits halfway.
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>4</beats><beat-type>4</beat-type></time></attributes>
          {p}{cresc}{c}{d}{stop}{f}{e}{c2}
        </measure>"#,
        p = dynamic("p"),
        cresc = wedge("crescendo"),
        c = note("C", 4, None, 1, ""),
        d = note("D", 4, None, 1, ""),
        stop = wedge("stop"),
        f = dynamic("f"),
        e = note("E", 4, None, 1, ""),
        c2 = note("C", 5, None, 1, "")
    ));
    assert_eq!(report.hairpins, 1);
    let p = 49.0 / 127.0;
    let f = 96.0 / 127.0;
    assert_eq!(
        amplitudes(&score.cells[0]),
        vec![Some(p), Some(p + (f - p) * 0.5), Some(f), Some(f)]
    );
}

#[test]
fn hairpin_without_target_moves_one_mark_level() {
    // A dim. with a stop but no following mark lands one level below p.
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>3</beats><beat-type>4</beat-type></time></attributes>
          {p}{c}{dim}{d}{stop}{e}
        </measure>"#,
        p = dynamic("p"),
        c = note("C", 4, None, 1, ""),
        dim = wedge("diminuendo"),
        d = note("D", 4, None, 1, ""),
        stop = wedge("stop"),
        e = note("E", 4, None, 1, "")
    ));
    let p = 49.0 / 127.0;
    let pp = 33.0 / 127.0;
    assert_eq!(
        amplitudes(&score.cells[0]),
        vec![Some(p), Some(p), Some(pp)]
    );
}

#[test]
fn hairpin_target_contradicting_direction_falls_back_one_level() {
    // dim … then a *louder* mark: the wedge must not rise into it.
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>3</beats><beat-type>4</beat-type></time></attributes>
          {p}{dim}{c}{stop}{d}{ff}{e}
        </measure>"#,
        p = dynamic("p"),
        dim = wedge("diminuendo"),
        c = note("C", 4, None, 1, ""),
        stop = wedge("stop"),
        d = note("D", 4, None, 1, ""),
        ff = dynamic("ff"),
        e = note("E", 4, None, 1, "")
    ));
    let p = 49.0 / 127.0;
    let pp = 33.0 / 127.0;
    let ff = 112.0 / 127.0;
    assert_eq!(
        amplitudes(&score.cells[0]),
        vec![Some(p), Some(pp), Some(ff)]
    );
}

#[test]
fn tied_notes_keep_the_amplitude_struck_at_their_first_onset() {
    // The tie starts under p; the mark changes mid-tie but the held chain
    // keeps its struck level (held steps never carry amplitude).
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {p}{tie_start}{ff}{tie_stop}
        </measure>"#,
        p = dynamic("p"),
        tie_start = note("C", 4, None, 1, r#"<tie type="start"/>"#),
        ff = dynamic("ff"),
        tie_stop = note("C", 4, None, 1, r#"<tie type="stop"/>"#)
    ));
    assert_eq!(shape(&score.cells[0]), "0 H");
    assert_eq!(amplitudes(&score.cells[0]), vec![Some(49.0 / 127.0), None]);
}

#[test]
fn accent_marks_warn_and_use_sound_dynamics_fallback() {
    // sfz names no level: with a <sound dynamics>, the percentage is used
    // (90 × pct / 100 / 127); without one it is skipped with a warning.
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          <direction><direction-type><dynamics><sfz/></dynamics></direction-type>
            <sound dynamics="100"/></direction>
          {c}
          <direction><direction-type><dynamics><sf/></dynamics></direction-type></direction>
          {d}
        </measure>"#,
        c = note("C", 4, None, 1, ""),
        d = note("D", 4, None, 1, "")
    ));
    assert_eq!(report.dynamic_marks, 1);
    assert!(report.warnings.iter().any(|w| w.contains("'sf'")), "{:?}", report.warnings);
    let sfz = 0.9 * 100.0 / 127.0;
    assert_eq!(amplitudes(&score.cells[0]), vec![Some(sfz), Some(sfz)]);
}

#[test]
fn dynamics_serialize_and_validate_as_score_v1() {
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {}{}{}
        </measure>"#,
        dynamic("pp"),
        note("C", 4, None, 1, ""),
        note("D", 4, None, 1, "")
    ));
    let json = score.to_json().expect("serializes");
    assert!(json.contains("\"amplitude\""), "{}", json);
    let reparsed = Score::from_json(&json).expect("round-trips through validation");
    assert_eq!(
        amplitudes(&reparsed.cells[0]),
        amplitudes(&score.cells[0])
    );
}

/// A `<direction>` carrying one `<pedal>` element of the given type.
fn pedal(kind: &str) -> String {
    format!(
        "<direction><direction-type><pedal type=\"{}\"/></direction-type></direction>",
        kind
    )
}

#[test]
fn pedal_span_becomes_gate_lane() {
    // Pedal down through measure 1, up at the measure 2 barline.
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>4</beats><beat-type>4</beat-type></time></attributes>
          {p}{c}{c}{c}{c}
        </measure>
        <measure number="2">
          {u}{c}{c}{c}{c}
        </measure>"#,
        p = pedal("start"),
        u = pedal("stop"),
        c = note("C", 4, None, 1, "")
    ));
    assert_eq!(report.pedal_events, 2);
    assert_eq!(report.pedal_lanes, 1);
    assert_eq!(score.pedal.len(), 1);
    assert_eq!(shape(&score.pedal[0]), "0 H H H . . . .");
}

#[test]
fn pedal_change_retakes_mid_span() {
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>4</beats><beat-type>4</beat-type></time></attributes>
          {p}{c}{c}{r}{c}{c}
        </measure>
        <measure number="2">
          {u}{c}{c}{c}{c}
        </measure>"#,
        p = pedal("start"),
        r = pedal("change"),
        u = pedal("stop"),
        c = note("C", 4, None, 1, "")
    ));
    assert_eq!(report.pedal_events, 3);
    // The retake ends the first span and strikes a new one at beat 3.
    assert_eq!(shape(&score.pedal[0]), "0 H 0 H . . . .");
}

#[test]
fn pedal_open_span_holds_to_end() {
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>4</beats><beat-type>4</beat-type></time></attributes>
          {p}{c}{c}{c}{c}
        </measure>"#,
        p = pedal("start"),
        c = note("C", 4, None, 1, "")
    ));
    assert_eq!(shape(&score.pedal[0]), "0 H H H");
}

#[test]
fn stray_pedal_stop_warns() {
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {u}{c}{c}
        </measure>"#,
        u = pedal("stop"),
        c = note("C", 4, None, 1, "")
    ));
    assert!(
        report.warnings.iter().any(|w| w.contains("pedal stop")),
        "{:?}",
        report.warnings
    );
    assert_eq!(shape(&score.pedal[0]), ". .");
}

#[test]
fn score_without_pedal_omits_field() {
    let (score, report) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {c}{c}
        </measure>"#,
        c = note("C", 4, None, 1, "")
    ));
    assert_eq!(report.pedal_events, 0);
    assert!(score.pedal.is_empty());
    let json = score.to_json().expect("serializes");
    assert!(!json.contains("\"pedal\""), "{}", json);
}

#[test]
fn pedal_lane_serializes_and_validates_as_score_v1() {
    let (score, _) = convert(&format!(
        r#"<measure number="1">
          <attributes><divisions>1</divisions>
            <time><beats>2</beats><beat-type>4</beat-type></time></attributes>
          {p}{c}{c}
        </measure>"#,
        p = pedal("start"),
        c = note("C", 4, None, 1, "")
    ));
    let json = score.to_json().expect("serializes");
    let reparsed = Score::from_json(&json).expect("round-trips through validation");
    assert_eq!(shape(&reparsed.pedal[0]), shape(&score.pedal[0]));
}
