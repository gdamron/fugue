//! The mixer on its declared table: its levels, pans and master are
//! requests the audio thread applies, a schedule ramps them without
//! allocating, and only the channels below its channel count exist.

use super::migrated::{block_with_locks_held, read, schedule, write};
use super::requests::counted_block;
use super::*;
use crate::ControlValue;

const MIXER: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "osc", "type": "oscillator", "config": { "waveform": "sine" } },
        { "id": "mix", "type": "mixer", "config": { "channel_count": 2, "levels": [0.5, 0.5] } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc", "from_port": "audio", "to": "mix", "to_port": "audio.0" },
        { "from": "mix", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
        { "from": "mix", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
    ]
}"#;

fn refusal(rig: &Rig, key: &str, value: ControlValue) -> String {
    let surface = rig.live.control_surfaces.lock().unwrap()["mix"].clone();
    surface.set_control(key, value).unwrap_err()
}

fn number(value: ControlValue) -> f32 {
    match value {
        ControlValue::Number(number) => number,
        other => panic!("a number, not {other:?}"),
    }
}

#[test]
fn mixer_writes_apply_on_the_audio_thread_and_read_back() {
    let mut rig = Rig::new(MIXER);
    rig.render(1);
    write(&rig, "mix", "level.1", 3.0.into());
    write(&rig, "mix", "pan.0", (-0.5).into());
    write(&rig, "mix", "master", 0.25.into());
    assert_eq!(read(&rig, "mix", "level.1"), 0.5.into(), "pending");

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(read(&rig, "mix", "level.1"), 2.0.into(), "clamped");
    assert_eq!(read(&rig, "mix", "pan.0"), (-0.5).into());
    assert_eq!(read(&rig, "mix", "master"), 0.25.into());
}

#[test]
fn only_the_channels_below_the_channel_count_are_listed_and_writable() {
    let rig = Rig::new(MIXER);
    let surface = rig.live.control_surfaces.lock().unwrap()["mix"].clone();
    let keys: Vec<String> = surface.controls().into_iter().map(|m| m.key).collect();
    assert_eq!(
        keys,
        [
            "channel_count",
            "master",
            "level.0",
            "level.1",
            "pan.0",
            "pan.1"
        ]
    );
    assert_eq!(read(&rig, "mix", "channel_count"), 2.0.into());
    assert_eq!(
        refusal(&rig, "level.2", 1.0.into()),
        "Unknown control: level.2: channel_count is 2, so only 'level.0' to 'level.1' exist"
    );
    assert!(refusal(&rig, "pan.63", 0.0.into()).starts_with("Unknown control: pan.63:"));
    assert!(surface.get_control("level.2").is_err());
    assert!(surface.automation("level.2").is_none());
    assert_eq!(
        refusal(&rig, "channel_count", 3.0.into()),
        "Control 'channel_count' is read-only"
    );
}

#[test]
fn a_scheduler_ramps_a_mixer_without_allocating() {
    let mut rig = Rig::new(MIXER);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "mix", "control": "level.0", "value": 1.0, "ramp_steps": 4 },
            { "at_step": 0, "module": "mix", "control": "pan.1", "value": 1.0, "ramp_steps": 4 },
            { "at_step": 1, "module": "mix", "control": "master", "value": 0.5 }
        ]}),
    );
    for gate in [1.0, 0.0, 1.0, 0.0] {
        rig.live.write_input("sched", "clock", gate).unwrap();
        assert_eq!(counted_block(&mut rig), (0, 0));
    }
    let level = number(read(&rig, "mix", "level.0"));
    assert!(level > 0.5 && level < 1.0, "{level}");
    let pan = number(read(&rig, "mix", "pan.1"));
    assert!(pan > 0.0 && pan < 1.0, "{pan}");
    assert_eq!(read(&rig, "mix", "master"), 0.5.into());
}

#[test]
fn a_mixer_block_renders_while_the_control_locks_are_held() {
    let mut rig = Rig::new(MIXER);
    schedule(
        &mut rig,
        serde_json::json!({ "schedule": [
            { "at_step": 0, "module": "mix", "control": "level.1", "value": 1.0, "ramp_steps": 4 }
        ]}),
    );
    write(&rig, "mix", "level.0", 0.9.into());
    rig.live.write_input("sched", "clock", 1.0).unwrap();

    assert_eq!(block_with_locks_held(&mut rig, &[]), Ok((0, 0)));
    assert_eq!(read(&rig, "mix", "level.0"), 0.9.into());
}
