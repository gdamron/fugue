use super::NotePreview;

fn bass(bpm: f32) -> String {
    serde_json::json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": bpm } },
            { "id": "seq", "type": "step_sequencer", "config": {
                "root_note": 48, "step_count": 4, "gate_length": 0.5,
                "pattern": [{ "note": 0 }, { "note": null }, { "note": 7 }, { "note": null }]
            }},
            { "id": "osc", "type": "oscillator" },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "clock", "from_port": "beat", "to": "seq", "to_port": "clock" },
            { "from": "seq", "from_port": "frequency", "to": "osc", "to_port": "frequency" },
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    })
    .to_string()
}

fn answer(document: &str, request: &str) -> serde_json::Value {
    serde_json::from_str(&NotePreview::run(document, request, 44_100).unwrap()).unwrap()
}

#[test]
fn lists_the_notes_that_start_in_the_range() {
    let answer = answer(&bass(120.0), r#"{"bars":2,"start_bar":1}"#);
    let notes = answer["notes"].as_array().unwrap();
    let starts: Vec<f64> = notes
        .iter()
        .map(|n| n["start_beats"].as_f64().unwrap())
        .collect();
    let pitches: Vec<u64> = notes.iter().map(|n| n["pitch"].as_u64().unwrap()).collect();
    assert_eq!(pitches, [48, 55, 48, 55]);
    for (got, want) in starts.iter().zip([4.0, 6.0, 8.0, 10.0]) {
        assert!((got - want).abs() < 0.05, "{starts:?}");
    }
    assert_eq!(answer["range"]["start_seconds"], 2.0);
    assert_eq!(answer["range"]["end_seconds"], 6.0);
    let length = notes[0]["end_beats"].as_f64().unwrap() - starts[0];
    assert!((length - 0.5).abs() < 0.05, "{length}");
    assert_eq!(answer["truncated"], false);
    assert_eq!(answer["limit_reached"], false);
}

#[test]
fn refuses_a_range_past_the_frame_limit_before_running() {
    let error = NotePreview::new(&bass(120.0), r#"{"bars":64,"start_bar":90}"#, 44_100)
        .err()
        .unwrap();
    assert_eq!(
        error,
        "the range needs 13671000 frames (310 seconds at 44100 Hz); \
         the largest allowed is 13230000 frames (300 seconds)"
    );
}

#[test]
fn refuses_a_document_with_the_words_loading_it_gives() {
    let mut engine = crate::RenderEngine::new(44_100);
    let loaded = engine.load_json("{nope").unwrap_err().to_string();
    assert_eq!(
        NotePreview::run("{nope", r#"{"bars":1}"#, 44_100).unwrap_err(),
        loaded
    );
    let unknown = NotePreview::run(&bass(120.0), r#"{"bars":1,"module_id":"ghost"}"#, 44_100);
    assert!(unknown.unwrap_err().contains("ghost"));
}

/// One voice sounding on every sixteenth: 16 notes a bar.
fn dense(id: &str) -> String {
    let pattern: Vec<_> = (0..16)
        .map(|i| serde_json::json!({ "note": i % 12 }))
        .collect();
    serde_json::json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
            { "id": id, "type": "step_sequencer", "config": {
                "root_note": 48, "step_count": 16, "gate_length": 0.5, "pattern": pattern
            }},
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "clock", "from_port": "beat_x4", "to": id, "to_port": "clock" }
        ]
    })
    .to_string()
}

/// Every page of `request`, raw.
fn pages(document: &str, request: serde_json::Value) -> Vec<String> {
    let mut pages = Vec::new();
    let mut request = request;
    loop {
        let raw = NotePreview::run(document, &request.to_string(), 44_100).unwrap();
        let page: serde_json::Value = serde_json::from_str(&raw).unwrap();
        pages.push(raw);
        match page["cursor"].as_str() {
            Some(cursor) => request["cursor"] = cursor.into(),
            None => return pages,
        }
    }
}

#[test]
fn pages_hold_at_most_256_notes_and_48_kib_and_chain_without_gaps() {
    let long = "a_module_name_long_enough_that_the_byte_cap_binds_before_the_note_cap";
    for id in ["dense", long] {
        let pages = pages(&dense(id), serde_json::json!({ "bars": 64 }));
        let mut starts = Vec::new();
        for raw in &pages {
            assert!(raw.len() <= 48 * 1024, "{} bytes", raw.len());
            let page: serde_json::Value = serde_json::from_str(raw).unwrap();
            let notes = page["notes"].as_array().unwrap();
            assert!(notes.len() <= 256);
            starts.extend(notes.iter().map(|n| n["start_seconds"].as_f64().unwrap()));
        }
        assert_eq!(starts.len(), 64 * 16, "{id}");
        for (i, start) in starts.iter().enumerate() {
            assert!(
                (start - i as f64 * 0.125).abs() < 0.03,
                "{id} note {i} at {start}"
            );
        }
        if id == long {
            assert!(pages.len() > 4, "the byte cap binds for a long id");
        }
    }
}

#[test]
fn a_cursor_fits_only_its_own_request() {
    let document = dense("dense");
    let first: serde_json::Value =
        serde_json::from_str(&NotePreview::run(&document, r#"{"bars":64}"#, 44_100).unwrap())
            .unwrap();
    let cursor = first["cursor"].as_str().unwrap();
    for request in [
        serde_json::json!({ "bars": 32, "cursor": cursor }),
        serde_json::json!({ "bars": 64, "seed": 1, "cursor": cursor }),
        serde_json::json!({ "bars": 64, "module_id": "dense", "cursor": cursor }),
    ] {
        let error = NotePreview::new(&document, &request.to_string(), 44_100)
            .err()
            .unwrap();
        assert!(error.contains("cursor"), "{error}");
    }
}

#[test]
fn lists_one_module_and_counts_two_second_bars_without_a_clock() {
    let only = answer(&bass(120.0), r#"{"bars":1,"module_id":"seq"}"#);
    assert_eq!(only["notes"].as_array().unwrap().len(), 2);
    let none = answer(&bass(120.0), r#"{"bars":1,"module_id":"osc"}"#);
    assert_eq!(none["notes"], serde_json::json!([]));

    let silent = serde_json::json!({
        "version": "1.0.0",
        "modules": [{ "id": "osc", "type": "oscillator" }, { "id": "dac", "type": "dac" }],
        "connections": []
    })
    .to_string();
    let answer = answer(&silent, r#"{"bars":3,"start_bar":2}"#);
    assert_eq!(answer["range"]["start_seconds"], 4.0);
    assert_eq!(answer["range"]["end_seconds"], 10.0);
    assert_eq!(answer["range"]["clock"], serde_json::Value::Null);
}

#[test]
fn refuses_a_range_too_late_to_count_and_a_clock_that_never_moves() {
    let absurd = format!(r#"{{"bars":1,"start_bar":{}}}"#, u64::MAX - 1);
    let error = NotePreview::new(&bass(120.0), &absurd, 44_100)
        .err()
        .unwrap();
    assert!(error.contains("start_bar"), "{error}");
    let late = format!(r#"{{"bars":64,"start_bar":{}}}"#, u32::MAX);
    let error = NotePreview::new(&bass(120.0), &late, 44_100).err().unwrap();
    assert!(error.contains("frames") && !error.contains("e+"), "{error}");
    let error = NotePreview::new(&bass(0.0), r#"{"bars":1}"#, 44_100)
        .err()
        .unwrap();
    assert!(error.contains("bpm"), "{error}");
}

#[test]
fn a_document_read_from_a_file_finds_its_assets_beside_it() {
    let dir = std::env::temp_dir().join(format!("preview-assets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pattern = serde_json::json!({ "pattern": [{ "note": 0 }, { "note": null }, { "note": 7 }, { "note": null }] });
    std::fs::write(dir.join("bass.json"), pattern.to_string()).unwrap();
    let mut document: serde_json::Value = serde_json::from_str(&bass(120.0)).unwrap();
    document["assets"] = serde_json::json!({ "bass": { "path": "bass.json" } });
    document["modules"][1]["config"]["pattern"] =
        serde_json::json!({ "$asset": "bass", "path": "/pattern" });
    let path = dir.join("piece.json");
    std::fs::write(&path, document.to_string()).unwrap();

    let invention = crate::Invention::from_file(path.to_str().unwrap()).unwrap();
    let preview = NotePreview::for_invention(invention, r#"{"bars":2,"start_bar":1}"#, 44_100);
    let read: serde_json::Value =
        serde_json::from_str(&preview.unwrap().answer().unwrap()).unwrap();
    let inline = answer(&bass(120.0), r#"{"bars":2,"start_bar":1}"#);
    assert_eq!(
        read["notes"], inline["notes"],
        "the same notes as the inline pattern"
    );
    // Without the file's place the asset is not found.
    assert!(NotePreview::new(&document.to_string(), r#"{"bars":2}"#, 44_100).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}
