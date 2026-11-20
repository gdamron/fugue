//! What a note preview answers for small pieces: chance and seeds, modules
//! it does not run, the range it counts, its limits, and its pages.

use fugue::NotePreview;
use serde_json::{json, Value};

const SAMPLE_RATE: u32 = 44_100;

/// One clock, one voice per lane (an oscillator and an ADSR into a VCA), a
/// mixer and a dac. A lane carries its id, type, config, clock port and
/// clock input.
fn piece(bpm: f64, beats_per_bar: Option<f64>, lanes: &[Value], extra: &[Value]) -> Value {
    let mut clock = json!({ "bpm": bpm });
    if let Some(beats) = beats_per_bar {
        clock["beats_per_bar"] = beats.into();
    }
    let mut modules = vec![json!({ "id": "clock", "type": "clock", "config": clock })];
    let mut connections = Vec::new();
    for (index, lane) in lanes.iter().enumerate() {
        let id = lane["id"].as_str().unwrap();
        modules.push(json!({ "id": id, "type": lane["type"], "config": lane["config"] }));
        connections.push(json!({
            "from": "clock", "from_port": lane["clock_port"],
            "to": id, "to_port": lane["clock_input"],
        }));
        let (osc, env, vca) = (
            format!("{id}_osc"),
            format!("{id}_env"),
            format!("{id}_vca"),
        );
        modules
            .push(json!({ "id": osc, "type": "oscillator", "config": { "waveform": "sawtooth" } }));
        modules.push(json!({ "id": env, "type": "adsr", "config": { "attack": 0.01, "decay": 0.1, "sustain": 0.6, "release": 0.15 } }));
        modules.push(json!({ "id": vca, "type": "vca" }));
        connections.extend([
            json!({ "from": id, "from_port": "frequency", "to": osc, "to_port": "frequency" }),
            json!({ "from": id, "from_port": "gate", "to": env, "to_port": "gate" }),
            json!({ "from": osc, "from_port": "audio", "to": vca, "to_port": "audio" }),
            json!({ "from": env, "from_port": "envelope", "to": vca, "to_port": "level" }),
            json!({ "from": vca, "from_port": "audio", "to": "mixer", "to_port": format!("audio.{index}") }),
        ]);
    }
    modules.push(json!({
        "id": "mixer", "type": "mixer",
        "config": { "channel_count": lanes.len(), "levels": vec![0.5; lanes.len()], "master": 0.8 },
    }));
    modules.push(json!({ "id": "dac", "type": "dac" }));
    connections.extend([
        json!({ "from": "mixer", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" }),
        json!({ "from": "mixer", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }),
    ]);
    modules.extend(extra.iter().cloned());
    json!({ "version": "1.0.0", "title": "preview test", "modules": modules, "connections": connections })
}

fn step_lane(id: &str, pattern: Value, clock_port: &str) -> Value {
    let steps = pattern.as_array().unwrap().len();
    json!({
        "id": id, "type": "step_sequencer", "clock_port": clock_port, "clock_input": "clock",
        "config": { "root_note": 48, "step_count": steps, "gate_length": 0.5, "pattern": pattern },
    })
}

fn cell_lane(id: &str, cell: Value, clock_port: &str) -> Value {
    let steps = cell.as_array().unwrap().len();
    json!({
        "id": id, "type": "cell_sequencer", "clock_port": clock_port, "clock_input": "clock",
        "config": { "root_note": 60, "step_count": steps, "gate_length": 0.9, "start_cell": 0, "cells": [cell] },
    })
}

fn melody_lane(id: &str) -> Value {
    json!({
        "id": id, "type": "melody", "clock_port": "beat", "clock_input": "gate",
        "config": { "root_note": 60 },
    })
}

/// The step sequencer example's bass line: 9 notes in 16 steps.
fn bass(extra: &[Value]) -> Value {
    let pattern = json!([
        { "note": 0, "gate_length": 0.8 }, { "note": null }, { "note": 0, "gate_length": 0.3 }, { "note": null },
        { "note": 0, "gate_length": 0.8 }, { "note": null }, { "note": 7, "gate_length": 0.5 }, { "note": null },
        { "note": 0, "gate_length": 0.8 }, { "note": null }, { "note": 0, "gate_length": 0.3 }, { "note": 5, "gate_length": 0.5 },
        { "note": 0, "gate_length": 0.8 }, { "note": null }, { "note": 3, "gate_length": 0.5 }, { "note": null },
    ]);
    piece(120.0, None, &[step_lane("seq", pattern, "beat")], extra)
}

fn melody() -> Value {
    piece(120.0, None, &[melody_lane("tune")], &[])
}

/// Eight steps with a note each, clocked on the beat.
fn plain_steps() -> Value {
    let pattern: Vec<Value> = (0..8).map(|i| json!({ "note": i })).collect();
    piece(
        120.0,
        None,
        &[step_lane("seq", pattern.into(), "beat")],
        &[],
    )
}

/// The same steps clocked by a chance module's first gate, so chance
/// decides which sixteenths sound and the notes come from the sequencer.
fn chance_steps() -> Value {
    let mut document = plain_steps();
    let connections = document["connections"].as_array_mut().unwrap();
    connections.retain(|c| !(c["from"] == "clock" && c["to"] == "seq"));
    connections.extend([
        json!({ "from": "clock", "from_port": "beat_x4", "to": "dice", "to_port": "clock" }),
        json!({ "from": "dice", "from_port": "gate_1", "to": "seq", "to_port": "clock" }),
    ]);
    document["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "dice", "type": "chance", "config": {} }));
    document
}

fn dense(id: &str) -> Value {
    let pattern: Vec<Value> = (0..16).map(|i| json!({ "note": i % 12 })).collect();
    piece(
        120.0,
        None,
        &[step_lane(id, pattern.into(), "beat_x4")],
        &[],
    )
}

fn raw(document: &Value, request: Value) -> Result<String, String> {
    NotePreview::run(&document.to_string(), &request.to_string(), SAMPLE_RATE)
}

fn answer(document: &Value, request: Value) -> Value {
    serde_json::from_str(&raw(document, request).unwrap()).unwrap()
}

fn notes(answer: &Value) -> Vec<Value> {
    answer["notes"].as_array().unwrap().clone()
}

fn every_page(document: &Value, mut request: Value) -> (usize, Vec<Value>) {
    let (mut pages, mut all) = (0, Vec::new());
    loop {
        let page = answer(document, request.clone());
        pages += 1;
        all.extend(notes(&page));
        match page["cursor"].as_str() {
            Some(cursor) => request["cursor"] = cursor.into(),
            None => return (pages, all),
        }
    }
}

fn key(note: &Value) -> String {
    format!(
        "{}|{}|{}|{}",
        note["start_seconds"], note["module_id"], note["voice"], note["pitch"]
    )
}

#[test]
fn a_chance_module_that_decides_the_notes_makes_them_one_possible_playing() {
    let first = answer(&chance_steps(), json!({ "bars": 8 }));
    assert!(!notes(&first).is_empty());
    assert!(notes(&first).iter().all(|n| n["module_id"] == "seq"));
    assert_eq!(first["chance"]["involved"], true);
    assert!(first["chance"]["modules"]
        .as_array()
        .unwrap()
        .contains(&json!("dice")));
    let message = first["chance"]["message"].as_str().unwrap();
    assert!(
        message.contains("one possible playing") && message.contains("seed"),
        "{message}"
    );

    let again = raw(&chance_steps(), json!({ "bars": 8, "seed": 11 })).unwrap();
    assert_eq!(
        raw(&chance_steps(), json!({ "bars": 8, "seed": 11 })).unwrap(),
        again
    );

    let shape = |seed: u64| -> Vec<(String, Value)> {
        notes(&answer(&chance_steps(), json!({ "bars": 8, "seed": seed })))
            .iter()
            .map(|n| (n["start_beats"].to_string(), n["pitch"].clone()))
            .collect()
    };
    let base = shape(0);
    let differing = [1, 2, 3, 4]
        .into_iter()
        .filter(|s| shape(*s) != base)
        .count();
    assert!(
        differing >= 3,
        "only {differing} of 4 other seeds changed the notes"
    );

    let plain = answer(&plain_steps(), json!({ "bars": 8 }));
    assert_eq!(plain["chance"]["involved"], false);
    assert_eq!(plain["chance"]["modules"], json!([]));
    let message = plain["chance"]["message"].as_str().unwrap();
    assert!(message.contains("chance") && !message.contains("one possible playing"));
    assert!(!message.contains("different seed"));
}

#[test]
fn the_seed_defaults_to_zero_is_echoed_and_moves_only_unseeded_chance() {
    let without = raw(&melody(), json!({ "bars": 8 })).unwrap();
    assert_eq!(
        raw(&melody(), json!({ "bars": 8, "seed": 0 })).unwrap(),
        without
    );
    assert_eq!(serde_json::from_str::<Value>(&without).unwrap()["seed"], 0);

    let pitches = |seed: u64| -> Vec<Value> {
        notes(&answer(&melody(), json!({ "bars": 8, "seed": seed })))
            .iter()
            .map(|n| n["pitch"].clone())
            .collect()
    };
    let base = pitches(0);
    let differing = (1..=8).filter(|s| pitches(*s) != base).count();
    assert!(
        differing >= 6,
        "only {differing} of 8 other seeds changed the melody"
    );

    let a = answer(&bass(&[]), json!({ "bars": 8, "seed": 1 }));
    let b = answer(&bass(&[]), json!({ "bars": 8, "seed": 2 }));
    assert_eq!(a["notes"], b["notes"]);
    assert_eq!((a["seed"].clone(), b["seed"].clone()), (json!(1), json!(2)));
    let largest = answer(
        &bass(&[]),
        json!({ "bars": 1, "seed": 9_007_199_254_740_991u64 }),
    );
    assert_eq!(largest["seed"], 9_007_199_254_740_991u64);
}

#[test]
fn chance_names_every_random_module_inside_developments_too() {
    let answer_for = |document: Value| answer(&document, json!({ "bars": 2 }));
    let melody_answer = answer_for(melody());
    assert_eq!(melody_answer["chance"]["involved"], true);
    assert_eq!(melody_answer["chance"]["modules"], json!(["tune"]));
    assert!(melody_answer["chance"]["message"]
        .as_str()
        .unwrap()
        .contains("seed"));
    let mixed = bass(&[
        json!({ "id": "dice", "type": "chance", "config": {} }),
        json!({ "id": "hiss", "type": "noise", "config": {} }),
    ]);
    let mixed = answer_for(mixed);
    assert_eq!(mixed["chance"]["involved"], true);
    let mut ids: Vec<String> = serde_json::from_value(mixed["chance"]["modules"].clone()).unwrap();
    ids.sort();
    assert_eq!(ids, ["dice", "hiss"]);
    let kinds = piece(
        120.0,
        None,
        &[melody_lane("tune_a"), melody_lane("tune_b")],
        &[json!({ "id": "dust", "type": "noise", "config": {} })],
    );
    let mut ids: Vec<String> =
        serde_json::from_value(answer_for(kinds)["chance"]["modules"].clone()).unwrap();
    ids.sort();
    assert_eq!(ids, ["dust", "tune_a", "tune_b"]);

    let mut wanderer = bass(&[json!({ "id": "inst", "type": "wanderer", "config": {} })]);
    wanderer["developments"] = json!([{ "name": "wanderer", "definition": {
        "version": "1.0.0", "title": "Wanderer",
        "modules": [{ "id": "inner_melody", "type": "melody", "config": { "root_note": 60 } }],
        "connections": [],
        "inputs": [{ "name": "gate", "to": "inner_melody", "to_port": "gate" }],
        "outputs": [{ "name": "frequency", "from": "inner_melody", "from_port": "frequency" }],
    }}]);
    let inner = answer_for(wanderer);
    assert_eq!(inner["chance"]["involved"], true);
    assert!(inner["chance"]["modules"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id.as_str().unwrap().contains("inner_melody")));
}

fn code(id: &str) -> Value {
    json!({ "id": id, "type": "code", "config": { "script": "function tick() {}" } })
}

fn agent(id: &str) -> Value {
    json!({ "id": id, "type": "agent", "config": { "system_prompt": "Play something.", "prompt": "Now." } })
}

#[test]
fn code_and_agent_modules_are_named_not_run_and_the_rest_still_plays() {
    let plain = answer(&bass(&[]), json!({ "bars": 8 }));
    assert_eq!(plain["not_run"], json!([]));
    let hosted = answer(
        &bass(&[code("script"), agent("player")]),
        json!({ "bars": 8 }),
    );
    let mut not_run = hosted["not_run"].as_array().unwrap().clone();
    not_run.sort_by_key(|m| m["module_id"].as_str().unwrap().to_owned());
    assert_eq!(
        not_run,
        [
            json!({ "module_id": "player", "type": "agent" }),
            json!({ "module_id": "script", "type": "code" }),
        ]
    );
    assert!(!notes(&plain).is_empty());
    assert_eq!(hosted["notes"], plain["notes"]);
    assert_eq!(hosted["truncated"], false);

    // An agent the clock triggers, and a code module beside it.
    let mut triggered = bass(&[code("watcher"), agent("conductor")]);
    triggered["connections"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "from": "clock", "from_port": "beat_d4", "to": "conductor", "to_port": "trigger",
        }));
    fugue::RenderEngine::new(SAMPLE_RATE)
        .load_json(&triggered.to_string())
        .expect("the triggered document loads");
    let answer_triggered = answer(&triggered, json!({ "bars": 8 }));
    let mut ids: Vec<String> = answer_triggered["not_run"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["module_id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    assert_eq!(ids, ["conductor", "watcher"]);
    assert!(!notes(&answer_triggered).is_empty());

    let mut nested = bass(&[json!({ "id": "holder", "type": "host_pair", "config": {} })]);
    nested["developments"] = json!([{ "name": "host_pair", "definition": {
        "version": "1.0.0", "title": "Host pair",
        "modules": [code("inner_script"), agent("inner_agent")], "connections": [],
    }}]);
    let inner = answer(&nested, json!({ "bars": 4 }));
    let type_of = |name: &str| {
        inner["not_run"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["module_id"].as_str().unwrap().contains(name))
            .map(|m| m["type"].clone())
    };
    assert_eq!(type_of("inner_script"), Some(json!("code")));
    assert_eq!(type_of("inner_agent"), Some(json!("agent")));
    assert!(!notes(&inner).is_empty());
}

#[test]
fn only_where_a_note_starts_counts_and_bars_follow_the_meter() {
    // One note held from step 0 for 20 sixteenths, still sounding in bar 1,
    // and a new one on step 20 (beat 5).
    let mut cell = vec![json!({ "note": null }); 64];
    cell[0] = json!({ "note": 0 });
    for step in cell.iter_mut().take(20).skip(1) {
        *step = json!({ "held": true });
    }
    cell[20] = json!({ "note": 4 });
    cell[21] = json!({ "held": true });
    let held = piece(120.0, None, &[cell_lane("a", cell.into(), "beat_x4")], &[]);
    let bar_one = answer(&held, json!({ "bars": 1, "start_bar": 1 }));
    let listed = notes(&bar_one);
    assert_eq!(
        listed
            .iter()
            .map(|n| n["pitch"].clone())
            .collect::<Vec<_>>(),
        [json!(64)]
    );
    assert!((listed[0]["start_beats"].as_f64().unwrap() - 5.0).abs() < 0.1);
    // Within a block and a half of 2.5 s, as the original allows.
    let block = 1024.0 / f64::from(SAMPLE_RATE);
    assert!((listed[0]["start_seconds"].as_f64().unwrap() - 2.5).abs() < block * 1.5);

    // A note still sounding a bar past the range has no duration; one that
    // ended has one.
    let mut long = vec![json!({ "held": true }); 64];
    long[0] = json!({ "note": 0 });
    for step in long.iter_mut().skip(40) {
        *step = json!({ "note": null });
    }
    long[44] = json!({ "note": 5 });
    let mut short = vec![json!({ "note": null }); 64];
    short[0] = json!({ "note": 7 });
    for step in short.iter_mut().take(6).skip(1) {
        *step = json!({ "held": true });
    }
    let lanes = [
        cell_lane("long", long.into(), "beat_x4"),
        cell_lane("short", short.into(), "beat_x4"),
    ];
    let durations = notes(&answer(
        &piece(120.0, None, &lanes, &[]),
        json!({ "bars": 1 }),
    ));
    let lane = |id: &str| {
        durations
            .iter()
            .filter(|n| n["module_id"] == id)
            .cloned()
            .collect::<Vec<_>>()
    };
    let (held_on, brief) = (lane("long"), lane("short"));
    assert_eq!(held_on.len(), 1);
    assert!(held_on[0]["duration_beats"].is_null() && held_on[0]["duration_seconds"].is_null());
    assert_eq!(brief.len(), 1);
    assert!((brief[0]["duration_beats"].as_f64().unwrap() - 1.5).abs() < 0.3);
    assert!(!brief[0]["duration_seconds"].is_null());

    let mut first = vec![json!({ "note": null }); 16];
    first[0] = json!({ "note": 0 });
    let triple = piece(
        120.0,
        Some(3.0),
        &[cell_lane("a", first.into(), "beat")],
        &[],
    );
    let range = &answer(&triple, json!({ "bars": 2, "start_bar": 1 }))["range"];
    assert_eq!(range["beats_per_bar"], 3.0);
    assert!((range["start_seconds"].as_f64().unwrap() - 1.5).abs() < 2e-3);
    assert!((range["end_seconds"].as_f64().unwrap() - 4.5).abs() < 2e-3);

    let mut soft = vec![json!({ "note": null }); 16];
    soft[0] = json!({ "note": 0, "velocity": 0.25 });
    soft[2] = json!({ "note": 2 });
    let velocities = notes(&answer(
        &piece(120.0, None, &[cell_lane("v", soft.into(), "beat")], &[]),
        json!({ "bars": 1 }),
    ));
    assert_eq!(velocities.len(), 2);
    assert!((velocities[0]["velocity"].as_f64().unwrap() - 0.25).abs() < 0.02);
    assert!((velocities[1]["velocity"].as_f64().unwrap() - 1.0).abs() < 0.02);
}

#[test]
fn limits_are_exact_at_their_edges() {
    let error = raw(&bass(&[]), json!({ "bars": 65 })).unwrap_err();
    assert!(
        error.contains("bars is 65; the largest allowed is 64"),
        "{error}"
    );
    let widest = answer(&bass(&[]), json!({ "bars": 64 }));
    assert_eq!(widest["range"]["bars"], 64);
    assert_eq!(
        widest["limits"],
        json!({ "max_bars": 64, "max_frames": 300 * 44_100 })
    );
    let at_48k: Value = serde_json::from_str(
        &NotePreview::run(&bass(&[]).to_string(), r#"{"bars":1}"#, 48_000).unwrap(),
    )
    .unwrap();
    assert_eq!(
        at_48k["limits"],
        json!({ "max_bars": 64, "max_frames": 300 * 48_000 })
    );

    let at_48k = NotePreview::run(
        &bass(&[]).to_string(),
        r#"{"bars":64,"start_bar":90}"#,
        48_000,
    )
    .unwrap_err();
    assert!(at_48k.contains(
        "the range needs 14880000 frames (310 seconds at 48000 Hz); the largest allowed is 14400000 frames (300 seconds)"
    ), "{at_48k}");
    // (85 + 64 + 1 bars) x 2 s is exactly 300 s.
    let edge = answer(&bass(&[]), json!({ "bars": 64, "start_bar": 85 }));
    assert_eq!(edge["range"]["start_bar"], 85);
    assert_eq!(edge["limit_reached"], false);
}

#[test]
fn one_module_that_plays_nothing_answers_empty_and_an_unknown_one_is_named() {
    let quiet = answer(&bass(&[]), json!({ "bars": 8, "module_id": "seq_osc" }));
    assert_eq!(quiet["notes"], json!([]));
    let error = raw(
        &bass(&[]),
        json!({ "bars": 8, "module_id": "no_such_voice" }),
    )
    .unwrap_err();
    assert!(error.contains("no_such_voice"), "{error}");
}

#[test]
fn pages_past_the_note_cap_and_the_byte_cap_chain_into_the_whole_range() {
    // 20 dense bars hold 320 notes, past the 256-note cap.
    let (pages, all) = every_page(&dense("dense"), json!({ "bars": 20 }));
    assert!(pages >= 2);
    assert_eq!(all.len(), 320);
    let unique: std::collections::HashSet<String> = all.iter().map(key).collect();
    assert_eq!(unique.len(), 320);

    // A 66-character id fills 48 KiB before 256 notes.
    let long = "a_rather_long_module_name_that_makes_every_note_bigger_than_usual_";
    let first = raw(&dense(long), json!({ "bars": 16 })).unwrap();
    let page: Value = serde_json::from_str(&first).unwrap();
    assert!(first.len() <= 48 * 1024);
    assert!(page["truncated"] == true && notes(&page).len() < 256);
    let (_, all) = every_page(&dense(long), json!({ "bars": 16 }));
    assert_eq!(all.len(), 256);
    let unique: std::collections::HashSet<String> = all.iter().map(key).collect();
    assert_eq!(unique.len(), 256);

    // Every page but the last is truncated with a cursor; the last has none.
    let mut request = json!({ "bars": 64 });
    loop {
        let page = answer(&dense("dense"), request.clone());
        match page["cursor"].as_str() {
            Some(cursor) => {
                assert_eq!(page["truncated"], true);
                request["cursor"] = cursor.into();
            }
            None => {
                assert_eq!(page["truncated"], false);
                break;
            }
        }
    }
    let small = answer(&dense("dense"), json!({ "bars": 2 }));
    assert_eq!(
        (small["truncated"].clone(), small["cursor"].clone()),
        (json!(false), Value::Null)
    );
    assert_eq!(notes(&small).len(), 32);

    // A cursor that is not one it gave, or one from another start bar, is named.
    let first = answer(&dense("dense"), json!({ "bars": 20 }));
    let cursor = first["cursor"].as_str().unwrap().to_owned();
    for request in [
        json!({ "bars": 20, "cursor": "not-a-cursor" }),
        json!({ "bars": 20, "start_bar": 1, "cursor": cursor }),
    ] {
        let error = raw(&dense("dense"), request).unwrap_err();
        assert!(error.contains("cursor"), "{error}");
    }

    // The halves of a range are the range.
    let (_, whole) = every_page(&dense("dense"), json!({ "bars": 64 }));
    let (_, left) = every_page(&dense("dense"), json!({ "bars": 32 }));
    let (_, right) = every_page(&dense("dense"), json!({ "bars": 32, "start_bar": 32 }));
    let keys = |notes: &[Value]| notes.iter().map(key).collect::<Vec<_>>();
    assert_eq!([keys(&left), keys(&right)].concat(), keys(&whole));
}
