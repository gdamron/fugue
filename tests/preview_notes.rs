//! A note preview lists what the piece plays live: the notes a threaded
//! backend's audio thread reports through `take_notes`, drained as the
//! daemon's `get_notes` drains them, are the preview's notes for the same
//! bars, frame for frame.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use fugue::{AudioBackend, InventionBuilder, NoteChange, NotePreview};
use serde_json::Value;

const SAMPLE_RATE: u32 = 44_100;

/// A device stand-in rendering 64-frame blocks on its own thread, up to
/// the frames the test allows, so the note ring never overflows between
/// two drains.
struct Device {
    allowed: Arc<AtomicU64>,
    rendered: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl AudioBackend for Device {
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn start(
        &mut self,
        mut render: Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (allowed, rendered, running) = (
            self.allowed.clone(),
            self.rendered.clone(),
            self.running.clone(),
        );
        running.store(true, Ordering::SeqCst);
        self.worker = Some(thread::spawn(move || {
            let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
            while running.load(Ordering::SeqCst) {
                if rendered.load(Ordering::SeqCst) + 64 <= allowed.load(Ordering::SeqCst) {
                    render(&mut left, &mut right);
                    rendered.fetch_add(64, Ordering::SeqCst);
                } else {
                    thread::sleep(Duration::from_micros(200));
                }
            }
        }));
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The notes a live run of `document` plays in its first `frames` frames,
/// as `take_notes` pairs them: each note once, with its end when it ended.
fn live_notes(document: &str, seed: u64, frames: u64) -> Vec<NoteChange> {
    // The device renders whole blocks.
    let frames = frames.div_ceil(64) * 64;
    let invention = serde_json::from_str(document).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE)
        .with_seed(seed)
        .build(invention)
        .unwrap();
    let allowed = Arc::new(AtomicU64::new(0));
    let rendered = Arc::new(AtomicU64::new(0));
    let device = Device {
        allowed: allowed.clone(),
        rendered: rendered.clone(),
        running: Arc::new(AtomicBool::new(false)),
        worker: None,
    };
    let running = runtime.start_without_hosts(device).unwrap();
    let mut notes: Vec<NoteChange> = Vec::new();
    let drain = |notes: &mut Vec<NoteChange>| {
        let changes = running.take_notes();
        assert_eq!(changes.missed, 0, "the live ring overflowed");
        for change in changes.notes {
            match notes
                .iter_mut()
                .find(|n| n.module_id == change.module_id && n.note == change.note)
            {
                Some(held) => held.end = change.end,
                None => notes.push(change),
            }
        }
    };
    while rendered.load(Ordering::SeqCst) < frames {
        let next = (allowed.load(Ordering::SeqCst) + 4096).min(frames);
        allowed.store(next, Ordering::SeqCst);
        while rendered.load(Ordering::SeqCst) + 64 <= next {
            thread::sleep(Duration::from_micros(200));
        }
        drain(&mut notes);
    }
    drain(&mut notes);
    running.stop();
    notes
}

/// The benchmark, BWV 772, with its score inlined and its piano
/// development given inline, since a test has no package catalog.
fn benchmark() -> String {
    let read = |name: &str| -> Value {
        let path = format!(
            "{}/tests/fixtures/preview_notes/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    let score = read("bwv772.score.json");
    fn inline(value: &mut Value, score: &Value) {
        match value {
            Value::Object(map) if map.get("$asset").is_some() => {
                let path = map["path"].as_str().unwrap_or("");
                *value = score.pointer(path).unwrap().clone();
            }
            Value::Object(map) => map.values_mut().for_each(|v| inline(v, score)),
            Value::Array(items) => items.iter_mut().for_each(|v| inline(v, score)),
            _ => {}
        }
    }
    let mut document = read("invention.json");
    inline(&mut document, &score);
    let map = document.as_object_mut().unwrap();
    map.remove("assets");
    map.insert(
        "developments".into(),
        serde_json::json!([{ "name": "piano", "definition": read("piano.voice.json") }]),
    );
    document.to_string()
}

fn example(name: &str) -> String {
    let path = format!("{}/examples/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap()
}

fn melody() -> String {
    let path = format!(
        "{}/tests/fixtures/preview_notes/melody.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap()
}

/// Every note of the preview's pages for `request`.
fn preview_notes(document: &str, request: Value) -> (Vec<Value>, Value) {
    let mut notes = Vec::new();
    let mut request = request;
    loop {
        let answer: Value = serde_json::from_str(
            &NotePreview::run(document, &request.to_string(), SAMPLE_RATE).unwrap(),
        )
        .unwrap();
        notes.extend(answer["notes"].as_array().unwrap().iter().cloned());
        match answer["cursor"].as_str() {
            Some(cursor) => request["cursor"] = cursor.into(),
            None => return (notes, answer),
        }
    }
}

/// Checks that the preview of the first `bars` bars lists exactly the live
/// notes that start in them, in the same frames.
fn matches_live(document: &str, seed: u64, bars: u64) {
    let (preview, answer) =
        preview_notes(document, serde_json::json!({ "bars": bars, "seed": seed }));
    let end_seconds = answer["range"]["end_seconds"].as_f64().unwrap();
    let beats_per_bar = answer["range"]["beats_per_bar"].as_f64().unwrap();
    let end_beats = bars as f64 * beats_per_bar;
    // Run live a bar past the range, as the preview does.
    let bar_seconds = end_seconds / bars as f64;
    let frames = ((end_seconds + bar_seconds) * f64::from(SAMPLE_RATE)).ceil() as u64 + 64;
    let mut live: Vec<NoteChange> = live_notes(document, seed, frames)
        .into_iter()
        .filter(|note| match note.start.beats {
            Some(beats) => beats + 1e-4 < end_beats,
            None => (note.start.frame as f64) < end_seconds * f64::from(SAMPLE_RATE),
        })
        .collect();
    live.sort_by(|a, b| {
        (a.start.frame, &a.module_id, a.voice).cmp(&(b.start.frame, &b.module_id, b.voice))
    });
    assert!(!live.is_empty(), "the live run played notes");
    assert_eq!(preview.len(), live.len(), "note count");
    let rate = f64::from(SAMPLE_RATE);
    for (got, want) in preview.iter().zip(&live) {
        let context = format!("{got} against {want:?}");
        assert_eq!(got["module_id"], want.module_id.as_str(), "{context}");
        assert_eq!(got["voice"], want.voice.to_string().as_str(), "{context}");
        assert_eq!(got["pitch"], want.pitch.round() as u64, "{context}");
        let start = got["start_seconds"].as_f64().unwrap();
        assert!(
            (start - want.start.frame as f64 / rate).abs() < 1e-6,
            "{context}"
        );
        assert_eq!(got["clock"].as_str(), want.clock.as_deref(), "{context}");
        let velocity = got["velocity"].as_f64().unwrap();
        assert!(
            (velocity - f64::from(want.velocity)).abs() < 1e-4,
            "{context}"
        );
        if let Some(beats) = want.start.beats {
            let got = got["start_beats"].as_f64().unwrap();
            assert!((got - beats).abs() < 1e-6, "{context}");
        }
        // Live runs at least as long as the preview, so a note that ended
        // live by then has a duration in the preview too.
        match want.end {
            None => assert!(got["duration_seconds"].is_null(), "{context}"),
            Some(end) if (end.frame as f64) < (end_seconds + bar_seconds) * rate => {
                assert!(!got["duration_seconds"].is_null(), "{context}");
            }
            Some(_) => {}
        }
        if let (Some(duration), Some(end)) = (got["duration_seconds"].as_f64(), want.end) {
            let live = (end.frame - want.start.frame) as f64 / rate;
            assert!((duration - live).abs() < 1e-6, "{context}");
        }
    }
}

#[test]
fn the_benchmark_lists_what_plays_live() {
    matches_live(&benchmark(), 0, 8);
    // 8 bars of 4 beats at the benchmark's 180 bpm, independent of the
    // preview's own reading of its clock.
    let (_, answer) = preview_notes(&benchmark(), serde_json::json!({ "bars": 8 }));
    let end = answer["range"]["end_seconds"].as_f64().unwrap();
    assert!((end - 8.0 * 4.0 * 60.0 / 180.0).abs() < 1e-5, "{end}");
}

#[test]
fn a_step_sequencer_lists_what_plays_live() {
    matches_live(&example("step_sequencer.json"), 0, 8);
}

#[test]
fn a_seeded_melody_lists_what_plays_live() {
    for seed in [0, 7] {
        matches_live(&melody(), seed, 8);
    }
}

#[test]
fn a_preview_leaves_nothing_running_and_repeats_exactly() {
    let document = melody();
    let request = r#"{"bars":4,"seed":3}"#;
    let first = NotePreview::run(&document, request, SAMPLE_RATE).unwrap();
    assert_eq!(
        NotePreview::run(&document, request, SAMPLE_RATE).unwrap(),
        first
    );
}

#[test]
fn the_benchmark_opens_with_the_subject_and_bar_one_starts_on_the_g() {
    // Bars count from 0, so beats [0, 4) hold the seven notes of the subject
    // after its sixteenth rest, and the G on beat 4.0 opens bar 1.
    let (notes, _) = preview_notes(&benchmark(), serde_json::json!({ "bars": 1 }));
    let upper: Vec<u64> = notes
        .iter()
        .filter(|n| n["module_id"] == "upper")
        .map(|n| n["pitch"].as_u64().unwrap())
        .collect();
    assert_eq!(upper, [60, 62, 64, 65, 62, 64, 60]);
    assert!(notes
        .iter()
        .all(|n| n["start_beats"].as_f64().unwrap() < 4.0 + 0.1));
    assert!((notes[0]["start_beats"].as_f64().unwrap() - 0.5).abs() < 0.1);
    let (next, _) = preview_notes(
        &benchmark(),
        serde_json::json!({ "bars": 1, "start_bar": 1 }),
    );
    let first_upper = next.iter().find(|n| n["module_id"] == "upper").unwrap();
    assert_eq!(first_upper["pitch"], 67);
}

#[test]
fn a_later_range_lists_what_the_whole_run_starts_there_in_order() {
    let document = benchmark();
    // The first 44 bars, checked against the live run.
    matches_live(&document, 0, 44);
    let (whole, _) = preview_notes(&document, serde_json::json!({ "bars": 44 }));
    for (start_bar, bars) in [(0, 8), (8, 8), (5, 1), (30, 14)] {
        let (notes, _) = preview_notes(
            &document,
            serde_json::json!({ "bars": bars, "start_bar": start_bar }),
        );
        assert!(!notes.is_empty());
        let (from, to) = (start_bar as f64 * 4.0, (start_bar + bars) as f64 * 4.0);
        let expected: Vec<&Value> = whole
            .iter()
            .filter(|n| {
                let beats = n["start_beats"].as_f64().unwrap();
                beats + 1e-4 >= from && beats + 1e-4 < to
            })
            .collect();
        assert_eq!(notes.len(), expected.len(), "bars {start_bar}+{bars}");
        for (got, want) in notes.iter().zip(expected) {
            for field in [
                "module_id",
                "pitch",
                "start_beats",
                "start_seconds",
                "velocity",
            ] {
                assert_eq!(got[field], want[field], "bars {start_bar}+{bars}");
            }
        }
        let key = |n: &Value| {
            (
                n["start_seconds"].as_f64().unwrap(),
                n["module_id"].as_str().unwrap().to_owned(),
                n["pitch"].as_u64().unwrap(),
            )
        };
        for pair in notes.windows(2) {
            let (a, b) = (key(&pair[0]), key(&pair[1]));
            let order =
                a.0.total_cmp(&b.0)
                    .then_with(|| a.1.cmp(&b.1))
                    .then_with(|| a.2.cmp(&b.2));
            assert!(
                order.is_le(),
                "{a:?} before {b:?} at bars {start_bar}+{bars}"
            );
        }
    }
}

#[test]
fn one_module_lists_exactly_its_own_notes() {
    let document = benchmark();
    let (all, _) = preview_notes(&document, serde_json::json!({ "bars": 8 }));
    let (lower, _) = preview_notes(
        &document,
        serde_json::json!({ "bars": 8, "module_id": "lower" }),
    );
    let expected: Vec<Value> = all
        .into_iter()
        .filter(|n| n["module_id"] == "lower")
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(lower, expected);
}

#[test]
fn the_benchmark_has_no_chance_and_names_its_main_clock() {
    let (_, answer) = preview_notes(&benchmark(), serde_json::json!({ "bars": 4 }));
    assert_eq!(answer["chance"]["involved"], false);
    assert_eq!(answer["chance"]["modules"], serde_json::json!([]));
    let message = answer["chance"]["message"].as_str().unwrap();
    assert!(message.contains("chance") && !message.contains("different seed"));
    assert_eq!(answer["range"]["clock"], "clock");
}

#[test]
fn a_bad_document_is_refused_with_the_words_loading_it_gives() {
    let raw_benchmark = std::fs::read_to_string(format!(
        "{}/tests/fixtures/preview_notes/invention.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let documents = [
        String::new(),
        r#"{"version":"1.0.0"}"#.to_owned(),
        serde_json::json!({ "version": "1.0.0", "connections": [],
            "modules": [{ "id": "a", "type": "no_such_module_type" }] })
        .to_string(),
        serde_json::json!({ "version": "1.0.0",
            "modules": [{ "id": "osc", "type": "oscillator" }, { "id": "dac", "type": "dac" }],
            "connections": [{ "from": "osc", "from_port": "nope", "to": "dac", "to_port": "audio" }] })
        .to_string(),
        serde_json::json!({ "version": "1.0.0", "modules": [{ "id": "dac", "type": "dac" }],
            "connections": [{ "from": "ghost", "from_port": "audio", "to": "dac", "to_port": "audio" }] })
        .to_string(),
        // The score still a file reference.
        raw_benchmark,
    ];
    for document in documents {
        let loaded = fugue::RenderEngine::new(SAMPLE_RATE)
            .load_json(&document)
            .unwrap_err()
            .to_string();
        let refused = NotePreview::run(&document, r#"{"bars":4}"#, SAMPLE_RATE).unwrap_err();
        assert_eq!(refused, loaded, "{document:.60}");
    }
}
