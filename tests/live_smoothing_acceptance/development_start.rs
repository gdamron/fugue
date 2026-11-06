//! A development instance whose config sets starting values plays them from
//! the first audio block: no glide from the inner modules' defaults.
//!
//! Each case wraps a small voice (a 187.5 Hz sine into a vca and/or a
//! filter) as a development exposing the inner controls, sets them in the
//! instance config, and compares what plays against the same wiring placed
//! flat with those values authored on the inner modules.

use super::*;

fn controls(entries: &[(&str, &str, &str)]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|(name, module, control)| {
                json!({ "name": name, "module": module, "control": control })
            })
            .collect(),
    )
}

/// The inner modules of a voice: the tone, then a vca and/or a low-pass
/// filter in series, each built from `inner` (the authored configs).
fn inner(vca: Option<Value>, filter: Option<Value>) -> (Vec<Value>, Vec<Value>, &'static str) {
    let mut modules = vec![tone("osc", json!({}))];
    let mut connections = Vec::new();
    let mut last = "osc";
    if let Some(config) = filter {
        modules.push(module("flt", "filter", config));
        connections.push(wire(last, "flt", "audio"));
        last = "flt";
    }
    if let Some(config) = vca {
        modules.push(module("amp", "vca", config));
        connections.push(wire(last, "amp", "audio"));
        last = "amp";
    }
    (modules, connections, last)
}

/// A main graph with one development instance `v` (config `instance`) of a
/// voice whose inner modules start at their defaults.
fn developed(vca: bool, filter: bool, exposed: &[(&str, &str, &str)], instance: Value) -> Value {
    let (modules, connections, last) = inner(
        vca.then(|| json!({})),
        filter.then(|| json!({ "filter_type": "lowpass" })),
    );
    let mut doc = document(
        vec![module("v", "voice", instance), dac()],
        vec![wire("v", "dac", "audio")],
    );
    doc["developments"] = json!([{
        "name": "voice",
        "definition": {
            "version": "1.0.0",
            "modules": modules,
            "connections": connections,
            "outputs": [{ "name": "audio", "from": last, "from_port": "audio" }],
            "controls": controls(exposed),
        }
    }]);
    doc
}

/// The same wiring flat in the main graph, the values authored inline.
fn flat(vca: Option<Value>, filter: Option<Value>) -> Value {
    let (mut modules, mut connections, last) = inner(vca, filter);
    modules.push(dac());
    connections.push(wire(last, "dac", "audio"));
    document(modules, connections)
}

struct Case {
    name: &'static str,
    developed: Value,
    flat: Value,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "vca level 0.25",
            developed: developed(
                true,
                false,
                &[("level", "amp", "level")],
                json!({ "level": 0.25 }),
            ),
            flat: flat(Some(json!({ "level": 0.25 })), None),
        },
        Case {
            name: "filter cutoff 100",
            developed: developed(
                false,
                true,
                &[("cutoff", "flt", "cutoff")],
                json!({ "cutoff": 100.0 }),
            ),
            flat: flat(
                None,
                Some(json!({ "filter_type": "lowpass", "cutoff": 100.0 })),
            ),
        },
        Case {
            name: "vca level 0.25 and filter cutoff 100",
            developed: developed(
                true,
                true,
                &[("level", "amp", "level"), ("cutoff", "flt", "cutoff")],
                json!({ "level": 0.25, "cutoff": 100.0 }),
            ),
            flat: flat(
                Some(json!({ "level": 0.25 })),
                Some(json!({ "filter_type": "lowpass", "cutoff": 100.0 })),
            ),
        },
        Case {
            // Nothing set: the inner defaults play, unchanged.
            name: "no starting values",
            developed: developed(true, true, &[("level", "amp", "level")], json!({})),
            flat: flat(Some(json!({})), Some(json!({ "filter_type": "lowpass" }))),
        },
    ]
}

/// The first 25 ms: a glide from a default to the set value would show.
const WINDOW: usize = 25 * MS / BLOCK * BLOCK;

#[test]
fn a_development_instance_plays_its_starting_values_from_the_first_block_offline() {
    let mut cases_ = Cases::default();
    for case in cases() {
        let want = render_offline(&case.flat, WINDOW);
        for block in [64usize, 128] {
            let mut engine = fugue::RenderEngine::new(RATE);
            engine
                .load_json(&case.developed.to_string())
                .expect("loads");
            engine.set_block_size(block);
            let mut buffer = vec![0.0f32; WINDOW * 2];
            engine.render_interleaved(&mut buffer).expect("renders");
            let got: Vec<f32> = buffer.chunks_exact(2).map(|f| f[0]).collect();
            cases_.check(
                format!("{} / block {block}", case.name),
                matches(&got, &want),
            );
        }
    }
    cases_.finish();
}

#[test]
fn a_development_instance_plays_its_starting_values_from_the_first_block_live() {
    let mut cases_ = Cases::default();
    for case in cases() {
        let want = render_offline(&case.flat, WINDOW);
        let got = Rig::start(Mode::Live, &case.developed).render(WINDOW);
        cases_.check(case.name, matches(&got, &want));
    }
    cases_.finish();
}

/// Equal to within rounding from sample 0, so any glide shows at once.
fn matches(got: &[f32], want: &[f32]) -> Result<(), String> {
    assert!(max_abs(want) > 0.01, "the flat render is audible");
    let off = max_difference(got, want, 0, want.len());
    if off <= 1e-4 {
        return Ok(());
    }
    let at = (0..want.len())
        .find(|&i| (got[i] - want[i]).abs() > 1e-4)
        .unwrap();
    Err(format!(
        "{off:.5} from the flat render, first at sample {at} (got {}, want {})",
        got[at], want[at]
    ))
}
