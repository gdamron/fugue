//! A step sequencer's velocities stay unheard while its `velocity` output is
//! unconnected: the line it plays, and its gate, are bit for bit the same
//! with or without stored velocities; and every first-party example that
//! plays one still sounds.

use std::path::{Path, PathBuf};
use std::thread;

use fugue::{Invention, RenderEngine};
use serde_json::json;

use super::rig::*;

/// A bass voice the sequencer plays without its velocity output.
fn voice(pattern: serde_json::Value) -> String {
    document(
        vec![
            clock(),
            sequencer(json!({
                "root_note": 48, "step_count": 8, "gate_length": 0.6, "pattern": pattern
            })),
            json!({ "id": "osc", "type": "oscillator", "config": { "waveform": "sawtooth" } }),
            json!({ "id": "adsr", "type": "adsr",
                    "config": { "attack": 0.005, "decay": 0.1, "sustain": 0.6, "release": 0.1 } }),
            json!({ "id": "vca", "type": "vca" }),
            dac(),
        ],
        &[
            ["clock", "beat", "seq", "clock"],
            ["seq", "frequency", "osc", "frequency"],
            ["seq", "gate", "adsr", "gate"],
            ["osc", "audio", "vca", "audio"],
            ["adsr", "envelope", "vca", "level"],
            ["vca", "audio", "dac", "audio_left"],
            ["seq", "gate", "dac", "audio_right"],
        ],
    )
}

#[test]
fn stored_velocities_change_nothing_while_the_output_is_unconnected() {
    // The same line with and without stored velocities, graces included:
    // with the output unconnected, the velocities are not heard, and the
    // frequency, gate and step it plays are the same.
    let with = json!([
        { "note": 0, "velocity": 0.2 },
        { "note": 7, "velocity": 0.9, "grace": [5] },
        { "note": null },
        { "note": 3, "velocity": 0.5 },
        { "held": true },
        { "note": 12, "velocity": 0.0 },
        { "note": 10, "velocity": 0.7, "grace": [8, 9] },
        { "note": 0 }
    ]);
    let without = json!([
        { "note": 0 },
        { "note": 7, "grace": [5] },
        { "note": null },
        { "note": 3 },
        { "held": true },
        { "note": 12 },
        { "note": 10, "grace": [8, 9] },
        { "note": 0 }
    ]);
    let frames = 17 * STEP;
    let heard = render(&voice(with), frames);
    let before = render(&voice(without), frames);
    assert!(
        before.0.iter().any(|x| x.abs() > 0.1),
        "the line is audible"
    );
    for (i, (got, want)) in heard.0.iter().zip(&before.0).enumerate() {
        assert_eq!(got.to_bits(), want.to_bits(), "audio sample {i}");
    }
    for (i, (got, want)) in heard.1.iter().zip(&before.1).enumerate() {
        assert_eq!(got.to_bits(), want.to_bits(), "gate sample {i}");
    }
}

/// Long enough for every example's pattern to play through at least once.
const EXAMPLE_SECONDS: usize = 8;

/// Every example document under `examples/` that contains a step sequencer,
/// except those whose agent runs an installed definition: they load only
/// with its package installed, and the agent module's own tests render them.
fn examples_with_a_step_sequencer() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![Path::new("examples").to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "json") {
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains("\"step_sequencer\"") && !runs_a_definition(&text) {
                    found.push(path);
                }
            }
        }
    }
    found.sort();
    found
}

/// Whether an agent in the document `text` names an agent definition.
fn runs_a_definition(text: &str) -> bool {
    let document: serde_json::Value = serde_json::from_str(text).unwrap();
    let modules = document["modules"].as_array().into_iter().flatten();
    modules
        .into_iter()
        .any(|module| module["type"] == "agent" && module["config"].get("definition").is_some())
}

/// The peak of a seeded render of the example at `path`.
fn example_peak(path: &Path) -> f32 {
    let name = path.display();
    let invention =
        Invention::from_file(path.to_str().unwrap()).unwrap_or_else(|err| panic!("{name}: {err}"));
    let mut engine = RenderEngine::new(RATE);
    engine.set_seed(Some(1));
    engine.stub_agent_backends(true);
    engine
        .load_invention(invention)
        .unwrap_or_else(|err| panic!("{name}: {err}"));
    let mut block = vec![0.0f32; 1024];
    let mut peak = 0.0f32;
    for _ in 0..(RATE as usize * EXAMPLE_SECONDS).div_ceil(512) {
        engine.render_interleaved(&mut block).unwrap();
        for sample in &block {
            peak = peak.max(sample.abs());
        }
    }
    peak
}

#[test]
fn every_example_with_a_step_sequencer_still_sounds() {
    let examples = examples_with_a_step_sequencer();
    assert!(!examples.is_empty(), "examples use a step sequencer");
    let peaks: Vec<(String, f32)> = thread::scope(|scope| {
        let workers: Vec<_> = examples
            .chunks(examples.len().div_ceil(4))
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|path| (path.display().to_string(), example_peak(path)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    });
    let silent: Vec<String> = peaks
        .iter()
        .filter(|(_, peak)| *peak <= 0.01)
        .map(|(name, peak)| format!("{name}: peak {peak}"))
        .collect();
    assert!(silent.is_empty(), "silent renders:\n{}", silent.join("\n"));
}
