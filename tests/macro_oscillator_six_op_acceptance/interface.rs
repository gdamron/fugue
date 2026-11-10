//! C13 (example invention).

use super::rig::*;
use super::*;
use std::path::Path;

/// Criterion C13: "An example invention in `examples/` uses
/// `model: "six_op_fm"` with a triggered phrase, loads, and renders audible,
/// finite audio."
/// Assumptions: "triggered" means some connection ends at that module's
/// `trigger` input; audible means a dac peak above 0.01 and RMS above 0.003
/// over 8 s.
#[test]
fn an_example_invention_plays_a_triggered_six_op_fm_phrase() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("examples/ exists") {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        for m in doc["modules"].as_array().into_iter().flatten() {
            if m["type"] != "macro_oscillator" || m["config"]["model"] != "six_op_fm" {
                continue;
            }
            let patched = doc["connections"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|c| c["to"] == m["id"] && c["to_port"] == "trigger");
            if patched {
                found.push((path.clone(), text.clone()));
            }
        }
    }
    assert!(
        !found.is_empty(),
        "no examples/*.json has a macro_oscillator with model \"six_op_fm\" and a patched trigger"
    );
    for (path, text) in found {
        let name = path.display();
        fugue::Invention::from_file(path.to_str().unwrap())
            .unwrap_or_else(|err| panic!("{name} loads as an invention: {err}"));
        let mut engine = fugue::RenderEngine::new(SR);
        engine
            .load_json(&text)
            .unwrap_or_else(|err| panic!("{name} loads into the engine: {err}"));
        let mut out = vec![0.0f32; seconds(8.0) * 2];
        engine
            .render_interleaved(&mut out)
            .unwrap_or_else(|err| panic!("{name} renders: {err}"));
        assert!(
            out.iter().all(|s| s.is_finite()),
            "{name}: non-finite audio"
        );
        assert!(
            peak(&out) > 0.01,
            "{name}: peak {} is not audible",
            peak(&out)
        );
        assert!(
            rms(&out) > 0.003,
            "{name}: rms {} is not audible",
            rms(&out)
        );
    }
}
