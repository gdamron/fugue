//! Acceptance tests for how a development schedules its inner graph.
//!
//! A development must behave as the same wiring placed flat in the main graph:
//! routes into one input sum, only a feedback loop steps sample by sample, and
//! the acyclic members run a whole block at a time.

mod development_scheduling_support;

use development_scheduling_support::*;
use fugue::{Invention, ModuleRegistry};
use serde_json::{json, Value};

const FRAMES: usize = 6144;

fn osc(id: &str, freq: f64, depth: f64) -> Value {
    json!({ "id": id, "type": "oscillator",
            "config": { "waveform": "sine", "frequency": freq, "frequency_mod_depth": depth } })
}

fn conn(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

fn dac() -> Value {
    json!({ "id": "dac", "type": "dac", "config": { "soft_clip": false } })
}

fn instance() -> Value {
    json!({ "id": DEV_ID })
}

fn audible(samples: &[f32]) {
    assert!(samples.iter().any(|s| s.abs() > 1e-4), "render was silent");
}

/// Renders the development form and the flat form at each block size and
/// asserts they match bit for bit. Returns the development render at block 64.
fn assert_matches_flat(def: &Value, modules: &[Value], conns: &[Value], label: &str) -> Vec<f32> {
    let mut at_64 = Vec::new();
    for block in [64usize, 37, 1] {
        let dev = offline(wrapped(def, modules, conns), FRAMES, block);
        let flat = offline(flat(def, modules, conns), FRAMES, block);
        audible(&flat);
        assert_identical(&dev, &flat, &format!("{label} block={block}"));
        if block == 64 {
            at_64 = dev;
        }
    }
    at_64
}

// -- fan-in -----------------------------------------------------------------

/// Two inner oscillators feed one inner vca `audio` input.
fn two_inner_routes() -> (Value, Vec<Value>, Vec<Value>) {
    let def = json!({
        "version": "1.0.0",
        "modules": [
            osc("osc_a", 330.0, 0.0),
            osc("osc_b", 440.0, 0.0),
            { "id": "mix", "type": "vca", "config": { "level": 0.5 } },
        ],
        "connections": [
            conn("osc_a", "audio", "mix", "audio"),
            conn("osc_b", "audio", "mix", "audio"),
        ],
        "outputs": [ { "name": "audio", "from": "mix", "from_port": "audio" } ],
    });
    let modules = vec![instance(), dac()];
    let conns = vec![conn(DEV_ID, "audio", "dac", "audio")];
    (def, modules, conns)
}

/// C1: two routes into the same inner input of a development sum, matching
/// the same wiring flat in the main graph (not last-route-wins).
#[test]
fn c1_two_inner_routes_into_one_input_sum() {
    let (def, modules, conns) = two_inner_routes();
    let rendered = assert_matches_flat(&def, &modules, &conns, "two inner routes");

    // Neither source alone is what a dropped route would leave behind.
    for kept in ["osc_a", "osc_b"] {
        let mut single = def.clone();
        let routes: Vec<Value> = single["connections"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["from"] == kept)
            .cloned()
            .collect();
        single["connections"] = Value::Array(routes);
        let alone = offline(flat(&single, &modules, &conns), FRAMES, 64);
        assert!(
            rendered
                .iter()
                .zip(&alone)
                .any(|(a, b)| a.to_bits() != b.to_bits()),
            "development render equals {kept} alone: a route was dropped"
        );
    }
}

/// C1: an external development input plus an inner route on the same inner
/// port sum, matching flat.
#[test]
fn c1_external_input_and_inner_route_on_one_port_sum() {
    let def = json!({
        "version": "1.0.0",
        "modules": [
            osc("tone", 220.0, 60.0),
            { "id": "wobble", "type": "lfo", "config": { "rate": 3.0, "waveform": "sine" } },
        ],
        "connections": [ conn("wobble", "bipolar", "tone", "frequency_mod") ],
        "inputs": [ { "name": "mod", "to": "tone", "to_port": "frequency_mod" } ],
        "outputs": [ { "name": "audio", "from": "tone", "from_port": "audio" } ],
    });
    let modules = vec![osc("ext", 7.0, 0.0), instance(), dac()];
    let conns = vec![
        conn("ext", "audio", DEV_ID, "mod"),
        conn(DEV_ID, "audio", "dac", "audio"),
    ];
    assert_matches_flat(&def, &modules, &conns, "external plus inner route");
}

// -- one loop plus acyclic members ------------------------------------------

/// osc1 <-> osc2 (the loop), osc1 -> filter -> vca (acyclic tail) with an
/// envelope from the external gate, and an acyclic head that modulates the
/// filter. No input port has more than one route, so this isolates scheduling
/// from fan-in.
fn loop_with_head_and_tail() -> (Value, Vec<Value>, Vec<Value>) {
    let def = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "head", "type": "vca", "config": { "level": 0.5 } },
            osc("osc1", 220.0, 80.0),
            osc("osc2", 110.0, 80.0),
            { "id": "filter", "type": "filter",
              "config": { "filter_type": "lowpass", "cutoff": 1200.0, "resonance": 0.4 } },
            { "id": "adsr", "type": "adsr",
              "config": { "attack": 0.005, "decay": 0.1, "sustain": 0.6, "release": 0.2 } },
            { "id": "out", "type": "vca", "config": { "level": 0.8 } },
        ],
        "connections": [
            conn("head", "audio", "filter", "cutoff_mod"),
            conn("osc1", "audio", "osc2", "frequency_mod"),
            conn("osc2", "audio", "osc1", "frequency_mod"),
            conn("osc1", "audio", "filter", "audio"),
            conn("filter", "audio", "out", "audio"),
            conn("adsr", "envelope", "out", "level"),
        ],
        "inputs": [
            { "name": "mod", "to": "head", "to_port": "audio" },
            { "name": "gate", "to": "adsr", "to_port": "gate" },
        ],
        "outputs": [ { "name": "audio", "from": "out", "from_port": "audio" } ],
    });
    let modules = vec![
        json!({ "id": "clock", "type": "clock", "config": { "bpm": 128.0, "gate_length": 0.4 } }),
        json!({ "id": "lfo", "type": "lfo", "config": { "rate": 4.0, "waveform": "sine" } }),
        instance(),
        dac(),
    ];
    let conns = vec![
        conn("clock", "beat", DEV_ID, "gate"),
        conn("lfo", "bipolar", DEV_ID, "mod"),
        conn(DEV_ID, "audio", "dac", "audio"),
    ];
    (def, modules, conns)
}

/// C2: a development with one feedback loop plus acyclic head and tail
/// renders sample-identical to the same wiring flat in the main graph, at
/// several block sizes.
#[test]
fn c2_loop_with_acyclic_head_and_tail_matches_flat() {
    let (def, modules, conns) = loop_with_head_and_tail();
    assert_matches_flat(&def, &modules, &conns, "loop with head and tail");
}

/// C2: a loop with only an acyclic tail (cross-FM pair into filter and vca).
#[test]
fn c2_loop_with_acyclic_tail_only_matches_flat() {
    let def = json!({
        "version": "1.0.0",
        "modules": [
            osc("osc1", 220.0, 80.0),
            osc("osc2", 110.0, 80.0),
            { "id": "filter", "type": "filter",
              "config": { "filter_type": "lowpass", "cutoff": 900.0, "resonance": 0.3 } },
            { "id": "out", "type": "vca", "config": { "level": 0.7 } },
        ],
        "connections": [
            conn("osc1", "audio", "osc2", "frequency_mod"),
            conn("osc2", "audio", "osc1", "frequency_mod"),
            conn("osc1", "audio", "filter", "audio"),
            conn("filter", "audio", "out", "audio"),
        ],
        "outputs": [ { "name": "audio", "from": "out", "from_port": "audio" } ],
    });
    let modules = vec![instance(), dac()];
    let conns = vec![conn(DEV_ID, "audio", "dac", "audio")];
    assert_matches_flat(&def, &modules, &conns, "loop with tail only");
}

// -- block-wise execution of acyclic members --------------------------------

const BLOCK: usize = 64;
const BLOCKS: usize = 96;

/// Probes at the head, inside the loop and in the tail of a development that
/// also has a feedback loop. Returns the stats in that order.
fn run_probed(loop_wiring: bool) -> Vec<std::sync::Arc<ProbeStats>> {
    let (registry, stats) = registry_with_probes(&["probe_head", "probe_loop", "probe_tail"]);
    let mut connections = vec![
        conn("head", "audio", "filter", "cutoff_mod"),
        conn("osc1", "audio", "mid", "audio"),
        conn("mid", "audio", "osc2", "frequency_mod"),
        conn("osc1", "audio", "tail", "audio"),
        conn("tail", "audio", "filter", "audio"),
    ];
    if loop_wiring {
        connections.push(conn("osc2", "audio", "osc1", "frequency_mod"));
    }
    let def = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "head", "type": "probe_head" },
            osc("osc1", 220.0, 80.0),
            osc("osc2", 110.0, 80.0),
            { "id": "mid", "type": "probe_loop" },
            { "id": "tail", "type": "probe_tail" },
            { "id": "filter", "type": "filter",
              "config": { "filter_type": "lowpass", "cutoff": 1200.0, "resonance": 0.4 } },
        ],
        "connections": connections,
        "inputs": [ { "name": "mod", "to": "head", "to_port": "audio" } ],
        "outputs": [ { "name": "audio", "from": "filter", "from_port": "audio" } ],
    });
    let modules = vec![
        json!({ "id": "lfo", "type": "lfo", "config": { "rate": 4.0, "waveform": "sine" } }),
        instance(),
        dac(),
    ];
    let conns = vec![
        conn("lfo", "bipolar", DEV_ID, "mod"),
        conn(DEV_ID, "audio", "dac", "audio"),
    ];
    let heard = live(registry, wrapped(&def, &modules, &conns), BLOCKS, BLOCK);
    audible(&heard);
    stats
}

/// C3: the acyclic members of a development with a loop (head and tail) run
/// block-wise; only the members inside the loop run one frame at a time.
#[test]
fn c3_acyclic_members_run_block_wise_loop_members_per_sample() {
    let stats = run_probed(true);
    let (head, mid, tail) = (&stats[0], &stats[1], &stats[2]);
    for (name, probe) in [("head", head), ("tail", tail)] {
        assert_eq!(
            probe.max_frames(),
            BLOCK,
            "acyclic {name} member never saw a whole block"
        );
        assert_eq!(
            probe.min_frames(),
            BLOCK,
            "acyclic {name} member was given a partial block"
        );
        assert_eq!(
            probe.calls(),
            BLOCKS,
            "acyclic {name} member was not processed once per block"
        );
    }
    assert_eq!(mid.max_frames(), 1, "loop member saw more than one frame");
    assert_eq!(mid.min_frames(), 1);
    assert_eq!(
        mid.calls(),
        BLOCKS * BLOCK,
        "loop member not stepped per sample"
    );
}

/// Control for the harness: with no loop the whole development runs
/// block-wise, which already holds. (The loop member is then acyclic too.)
#[test]
fn c3_development_without_a_loop_runs_block_wise() {
    let stats = run_probed(false);
    for (i, probe) in stats.iter().enumerate() {
        assert_eq!(probe.max_frames(), BLOCK, "probe {i}");
        assert_eq!(probe.calls(), BLOCKS, "probe {i}");
    }
}

// -- live and offline -------------------------------------------------------

/// C7: live playback and offline render of a development with a loop and an
/// acyclic tail agree sample for sample at the same block size.
#[test]
fn c7_live_and_offline_agree_for_a_loop_with_acyclic_members() {
    let (def, modules, conns) = loop_with_head_and_tail();
    let blocks = FRAMES / BLOCK;
    let rendered = offline(wrapped(&def, &modules, &conns), blocks * BLOCK, BLOCK);
    let heard = live(
        ModuleRegistry::default(),
        wrapped(&def, &modules, &conns),
        blocks,
        BLOCK,
    );
    audible(&rendered);
    assert_identical(&rendered, &heard, "live vs offline");
}

// -- existing developments --------------------------------------------------

/// A clock and a seeded melody driving the instance's `frequency` and `gate`.
/// Every input the voices read from these is connected: a development input
/// left unconnected holds its inner port at zero, where the same port flat
/// falls back to the module's own default, so unconnected inputs are not
/// comparable with flat.
fn melody_driven(bpm: f64) -> (Vec<Value>, Vec<Value>) {
    let modules = vec![
        json!({ "id": "clock", "type": "clock", "config": { "bpm": bpm, "gate_length": 0.5 } }),
        json!({ "id": "melody", "type": "melody",
                "config": { "root_note": 50, "degrees": [0, 3, 5, 7], "seed": 11 } }),
        instance(),
        dac(),
    ];
    let conns = vec![
        conn("clock", "beat", "melody", "gate"),
        conn("melody", "frequency", DEV_ID, "frequency"),
        conn("melody", "gate", DEV_ID, "gate"),
        conn(DEV_ID, "audio", "dac", "audio"),
    ];
    (modules, conns)
}

/// C4: each curated voice development renders identically to its modules
/// placed flat in the main graph, driven by a clock gate.
#[test]
fn c4_curated_voice_developments_match_flat() {
    for name in ["piano", "marimba", "vibraphone", "pluck", "pad"] {
        let path = format!(
            "{}/examples/developments/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let def: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Parses as a development, as the library registers it.
        let parsed: Invention = serde_json::from_value(def.clone()).unwrap();
        assert!(parsed.is_development(), "{name} should be a development");

        let (modules, conns) = melody_driven(140.0);
        assert_matches_flat(&def, &modules, &conns, name);
    }
}

/// C4: the file-based example voice (no feedback) renders like its flat form.
#[test]
fn c4_example_development_voice_matches_flat() {
    let path = format!(
        "{}/examples/development_voice.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let def: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let (modules, conns) = melody_driven(96.0);
    assert_matches_flat(&def, &modules, &conns, "development_voice");
}
