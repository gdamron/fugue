use crate::ControlValue;

#[test]
fn render_engine_full_snapshot_includes_ports_and_control_values() {
    let json = r#"{
        "version": "1.0.0",
        "modules": [
            { "id": "osc", "type": "oscillator", "config": { "frequency": 440.0 } },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    }"#;
    let mut engine = crate::RenderEngine::new(44_100);
    engine.load_json(json).unwrap();
    engine
        .set_control("osc", "frequency", ControlValue::Number(880.0))
        .unwrap();

    let snapshot = engine.full_snapshot();
    assert_eq!(snapshot.status.module_count, 2);
    assert_eq!(snapshot.connections.len(), 1);

    let osc = snapshot
        .modules
        .iter()
        .find(|module| module.info.id == "osc")
        .expect("oscillator module is present");
    assert!(osc.ports.outputs.contains(&"audio".to_string()));
    assert!(osc.ports.inputs.contains(&"frequency".to_string()));
    let frequency = osc
        .controls
        .iter()
        .find(|control| control.meta.key == "frequency")
        .expect("frequency control is present");
    assert_eq!(frequency.value, Some(ControlValue::Number(880.0)));
}
