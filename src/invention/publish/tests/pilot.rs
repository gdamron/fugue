//! The first modules on declared controls, live from the graph's start:
//! writes are requests the audio thread applies without allocating, and
//! read back once applied.

use super::requests::counted_block;
use super::*;
use crate::ControlValue;

const PATCH: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc", "type": "oscillator", "config": { "frequency": 440.0 } },
        { "id": "vca", "type": "vca", "config": { "level": 1.0 } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio" },
        { "from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

fn read(rig: &Rig, id: &str, key: &str) -> ControlValue {
    let surfaces = rig.live.control_surfaces.lock().unwrap();
    surfaces[id].get_control(key).unwrap()
}

fn write(rig: &Rig, id: &str, key: &str, value: ControlValue) {
    let surface = rig.live.control_surfaces.lock().unwrap()[id].clone();
    surface.set_control(key, value).unwrap();
}

#[test]
fn vca_writes_apply_on_the_audio_thread_and_read_back() {
    let mut rig = Rig::new(PATCH);
    rig.render(1);
    write(&rig, "vca", "level", 2.0.into());
    assert_eq!(read(&rig, "vca", "level"), 1.0.into(), "pending");
    write(&rig, "vca", "level", 0.25.into());

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(read(&rig, "vca", "level"), 0.25.into());
    let peak = rig
        .render(4)
        .iter()
        .fold(0.0f32, |peak, v| peak.max(v.abs()));
    assert!(peak > 0.2 && peak < 0.2501, "{peak}");

    write(&rig, "vca", "level", (-1.0).into());
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(
        read(&rig, "vca", "level"),
        0.0.into(),
        "as the vca clamped it"
    );
    assert!(rig.render(1).iter().all(|v| *v == 0.0));
}
