use super::{CodeModuleRuntimeInfo, RenderEngine};
use crate::ControlValue;
use std::time::Duration;

const SIMPLE_INVENTION: &str = r#"{
    "version": "1.0.0",
    "title": "render-test",
    "modules": [
        { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
        { "id": "vca", "type": "vca", "config": { "level": 0.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio" },
        { "from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

#[test]
fn render_engine_renders_interleaved_audio() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(SIMPLE_INVENTION).unwrap();
    engine
        .set_control("vca", "cv", ControlValue::Number(0.5))
        .unwrap();

    let mut output = [0.0f32; 16];
    let frames = engine.render_interleaved(&mut output).unwrap();

    assert_eq!(frames, 8);
    assert!(output.iter().any(|sample| sample.abs() > 0.0));
}

#[test]
fn render_engine_reset_restores_state() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(SIMPLE_INVENTION).unwrap();
    engine
        .set_control("vca", "cv", ControlValue::Number(0.0))
        .unwrap();

    let mut silent = [0.0f32; 8];
    engine.render_interleaved(&mut silent).unwrap();

    engine
        .set_control("vca", "cv", ControlValue::Number(0.8))
        .unwrap();
    engine.reset().unwrap();

    let level = engine.get_control("vca", "cv").unwrap();
    assert_eq!(level, ControlValue::Number(1.0));
}

#[test]
fn render_engine_supports_runtime_graph_mutation() {
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
            ],
            "connections": []
        }"#,
        )
        .unwrap();

    engine
        .add_module(
            "osc",
            "oscillator",
            &serde_json::json!({ "waveform": "sine", "frequency": 440.0 }),
        )
        .unwrap();
    engine
        .add_module("vca", "vca", &serde_json::json!({ "level": 0.0 }))
        .unwrap();
    engine.connect("osc", "audio", "vca", "audio").unwrap();
    engine.connect("vca", "audio", "dac", "audio").unwrap();
    engine
        .set_control("vca", "cv", ControlValue::Number(0.5))
        .unwrap();

    assert_eq!(engine.list_modules().len(), 3);
    assert_eq!(engine.list_connections().len(), 2);

    let mut output = [0.0f32; 16];
    engine.render_interleaved(&mut output).unwrap();
    assert!(output.iter().any(|sample| sample.abs() > 0.0));
}

#[test]
fn render_engine_runs_code_module_init_hook() {
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "code1",
                    "type": "code",
                    "config": {
                        "script": "function init() { graph.addModule('osc_from_code', 'oscillator', { waveform: 'sine', frequency: 330.0 }) }"
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
        )
        .unwrap();

    std::thread::sleep(Duration::from_millis(50));
    assert!(engine
        .list_modules()
        .into_iter()
        .any(|module| module.id == "osc_from_code"));
}

#[test]
fn render_engine_lists_code_module_runtime_info() {
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "code1",
                    "type": "code",
                    "config": {
                        "script": "function init() {}",
                        "entrypoint": "init",
                        "enabled": true,
                        "tick_hz": 8.0
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
        )
        .unwrap();

    let modules = engine.list_code_modules().unwrap();
    assert_eq!(modules.len(), 1);
    assert!(matches!(
        &modules[0],
        CodeModuleRuntimeInfo {
            id,
            entrypoint,
            enabled,
            tick_hz,
            ..
        } if id == "code1" && entrypoint == "init" && *enabled && (*tick_hz - 8.0).abs() < f32::EPSILON
    ));
}

#[test]
fn render_engine_supports_returned_lifecycle_object() {
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "code1",
                    "type": "code",
                    "config": {
                        "script": "(() => ({ init() { graph.addModule('osc_from_object', 'oscillator', { waveform: 'sine', frequency: 440.0 }) } }))()"
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
        )
        .unwrap();

    std::thread::sleep(Duration::from_millis(50));
    assert!(engine
        .list_modules()
        .into_iter()
        .any(|module| module.id == "osc_from_object"));
}

#[test]
fn render_engine_supports_custom_entrypoint_function() {
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "code1",
                    "type": "code",
                    "config": {
                        "entrypoint": "boot",
                        "script": "function boot() { graph.addModule('osc_from_boot', 'oscillator', { waveform: 'sine', frequency: 660.0 }) }"
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
        )
        .unwrap();

    std::thread::sleep(Duration::from_millis(50));
    assert!(engine
        .list_modules()
        .into_iter()
        .any(|module| module.id == "osc_from_boot"));
}

#[test]
fn render_engine_keeps_legacy_globalthis_hooks_working() {
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "code1",
                    "type": "code",
                    "config": {
                        "script": "globalThis.init = function () { graph.addModule('osc_from_legacy', 'oscillator', { waveform: 'sine', frequency: 550.0 }) }"
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
        )
        .unwrap();

    std::thread::sleep(Duration::from_millis(50));
    assert!(engine
        .list_modules()
        .into_iter()
        .any(|module| module.id == "osc_from_legacy"));
}

/// Clock at 22_500 BPM = 128 samples per beat at 48 kHz; a 4-step one_shot
/// pattern therefore ends exactly at frame 4 * 128 = 512.
const ONE_SHOT_INVENTION: &str = r#"{
    "version": "1.0.0",
    "title": "one-shot-render-test",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 22500.0 } },
        {
            "id": "seq",
            "type": "step_sequencer",
            "config": {
                "steps": 4,
                "mode": "one_shot",
                "pattern": [ { "note": 0 }, { "note": 2 }, { "note": 4 }, { "note": 5 } ]
            }
        },
        { "id": "osc", "type": "oscillator", "config": { "waveform": "sine" } },
        { "id": "vca", "type": "vca" },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "clock", "from_port": "gate", "to": "seq", "to_port": "gate" },
        { "from": "seq", "from_port": "frequency", "to": "osc", "to_port": "frequency" },
        { "from": "seq", "from_port": "gate", "to": "vca", "to_port": "cv" },
        { "from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio" },
        { "from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// Renders block-by-block until the end gate fires; returns the absolute end
/// frame.
fn render_until_end(engine: &mut RenderEngine, source: Option<&str>) -> usize {
    let block = engine.block_size();
    let mut buffer = vec![0.0f32; block * 2];
    let mut done = 0usize;
    loop {
        engine.render_interleaved(&mut buffer).unwrap();
        if let Some(frame) = engine.scan_end_gate(source, block).unwrap() {
            return done + frame;
        }
        done += block;
        assert!(done < 48_000, "end gate never fired");
    }
}

#[test]
fn scan_end_gate_finds_the_exact_end_frame() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(ONE_SHOT_INVENTION).unwrap();

    // The clock pre-increments its sample counter before computing outputs,
    // so beat edges land at frames 0, 127, 255, 383, 511 (the first period
    // is one sample shorter). The 5th edge — the one that completes the
    // 4-step one_shot pattern — is therefore frame 511.
    let end_frame = render_until_end(&mut engine, None);
    assert_eq!(end_frame, 511, "4 steps at 128 samples per beat");

    // Naming the source module yields the same result.
    engine.reset().unwrap();
    let end_frame = render_until_end(&mut engine, Some("seq"));
    assert_eq!(end_frame, 511);
}

#[test]
fn scan_end_gate_rejects_bad_sources_and_endless_graphs() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(ONE_SHOT_INVENTION).unwrap();
    let block = engine.block_size();
    let mut buffer = vec![0.0f32; block * 2];
    engine.render_interleaved(&mut buffer).unwrap();

    let err = engine.scan_end_gate(Some("nope"), block).unwrap_err();
    assert!(err.to_string().contains("unknown end source"), "{}", err);
    let err = engine.scan_end_gate(Some("osc"), block).unwrap_err();
    assert!(err.to_string().contains("has no 'end' output"), "{}", err);

    // A graph with no end-capable module refuses --to-end semantics.
    let mut endless = RenderEngine::new(48_000);
    endless.load_json(SIMPLE_INVENTION).unwrap();
    endless.render_interleaved(&mut buffer).unwrap();
    let err = endless.scan_end_gate(None, block).unwrap_err();
    assert!(err.to_string().contains("never stop"), "{}", err);
}

#[test]
fn end_gate_render_is_deterministic() {
    let render_once = || {
        let mut engine = RenderEngine::new(48_000);
        engine.load_json(ONE_SHOT_INVENTION).unwrap();
        let block = engine.block_size();
        let mut buffer = vec![0.0f32; block * 2];
        let mut collected = Vec::new();
        for _ in 0..(1024 / block).max(8) {
            engine.render_interleaved(&mut buffer).unwrap();
            collected.extend_from_slice(&buffer);
        }
        collected
    };
    assert_eq!(render_once(), render_once(), "same invention, same bytes");
}

#[test]
fn end_reached_observes_the_ended_control() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(ONE_SHOT_INVENTION).unwrap();
    let block = engine.block_size();
    let mut buffer = vec![0.0f32; block * 2];

    assert!(!engine.end_reached(None).unwrap(), "not ended at load");
    assert!(!engine.end_reached(Some("seq")).unwrap());

    // Render past the piece's end (frame 511) and observe via controls.
    for _ in 0..(1024 / block).max(16) {
        engine.render_interleaved(&mut buffer).unwrap();
    }
    assert!(engine.end_reached(None).unwrap(), "ended after playthrough");
    assert!(engine.end_reached(Some("seq")).unwrap());

    // Error surfaces mirror scan_end_gate's.
    let err = engine.end_reached(Some("osc")).unwrap_err();
    assert!(err.contains("no 'ended' control"), "{}", err);
    let err = engine.end_reached(Some("nope")).unwrap_err();
    assert!(err.contains("unknown end source"), "{}", err);

    let mut endless = RenderEngine::new(48_000);
    endless.load_json(SIMPLE_INVENTION).unwrap();
    let err = endless.end_reached(None).unwrap_err();
    assert!(err.contains("never stop"), "{}", err);
}
