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
