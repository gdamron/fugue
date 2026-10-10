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

#[test]
fn a_scheduled_vca_jump_past_its_range_reads_as_the_vca_will_hold_it() {
    let built = crate::ModuleRegistry::default()
        .build("vca", 48_000, &serde_json::json!({}))
        .unwrap();
    let level = built.control_surface.unwrap().automation("level").unwrap();
    level.write_number(2.0);
    // A ramp starting here starts from 1, as the old clamping setter left it.
    assert_eq!(level.current(), Some(1.0));
}

#[test]
fn oscillator_writes_apply_on_the_audio_thread_and_read_back() {
    let mut rig = Rig::new(PATCH);
    rig.render(1);
    write(&rig, "osc", "frequency", 12_000.0.into());
    write(&rig, "osc", "waveform", "square".into());
    write(&rig, "vca", "level", 2.0.into());
    assert_eq!(read(&rig, "osc", "waveform"), "sine".into(), "pending");

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(read(&rig, "osc", "frequency"), 12_000.0.into());
    assert_eq!(read(&rig, "osc", "waveform"), "square".into());
    assert_eq!(
        read(&rig, "vca", "level"),
        1.0.into(),
        "as the vca clamped it"
    );
    let out = rig.render(1);
    assert!(
        out.iter().all(|v| v.abs() == 1.0),
        "a full-scale square: {out:?}"
    );

    write(&rig, "vca", "level", 0.5.into());
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert!(rig.render(1).iter().all(|v| v.abs() == 0.5));
}

#[test]
fn a_scheduler_ramps_an_oscillator_without_allocating() {
    let mut rig = Rig::new(PATCH);
    let schedule = serde_json::json!({ "schedule": [
        { "at_step": 0, "module": "osc", "control": "frequency", "value": 880.0, "ramp_steps": 4 }
    ]});
    let scheduler = rig.build("sched", "control_scheduler", schedule);
    rig.live
        .edit(|change| {
            change.upsert("sched", scheduler);
            Ok(())
        })
        .unwrap();
    rig.render(1);
    for gate in [1.0, 0.0, 1.0, 0.0] {
        rig.live.write_input("sched", "clock", gate).unwrap();
        assert_eq!(counted_block(&mut rig), (0, 0));
    }
    let ControlValue::Number(frequency) = read(&rig, "osc", "frequency") else {
        panic!("a number");
    };
    assert!(frequency > 440.0 && frequency < 880.0, "{frequency}");
}

#[test]
fn a_scheduled_oscillator_write_past_its_range_reads_as_it_will_hold_it() {
    let built = crate::ModuleRegistry::default()
        .build("oscillator", 48_000, &serde_json::json!({}))
        .unwrap();
    let surface = built.control_surface.unwrap();
    let depth = surface.automation("amplitude_mod_depth").unwrap();
    depth.write_number(2.0);
    assert_eq!(depth.current(), Some(1.0));
    let frequency = surface.automation("frequency").unwrap();
    frequency.write_number(-5.0);
    assert_eq!(frequency.current(), Some(0.0));
}

#[test]
fn an_oscillator_takes_the_waveform_spellings_it_always_took() {
    let registry = crate::ModuleRegistry::default();
    for (spelled, read) in [
        ("saw", "sawtooth"),
        ("TRI", "triangle"),
        ("Square", "square"),
    ] {
        let config = serde_json::json!({ "waveform": spelled });
        let built = registry.build("oscillator", 48_000, &config).unwrap();
        let surface = built.control_surface.unwrap();
        assert_eq!(surface.get_control("waveform").unwrap(), read.into());
        surface.set_control("waveform", spelled.into()).unwrap();
    }
    let mut rig = Rig::new(PATCH);
    rig.render(1);
    write(&rig, "osc", "waveform", "saw".into());
    rig.render(1);
    assert_eq!(read(&rig, "osc", "waveform"), "sawtooth".into());
}

#[test]
fn clock_writes_apply_on_the_audio_thread_and_retime_its_gates() {
    // 22500 bpm at 48 kHz is 128 samples a beat; 45000 is 64.
    let mut rig = Rig::new(
        r#"{
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 22500.0, "gate_length": 0.5 } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [{ "from": "clock", "from_port": "beat", "to": "dac", "to_port": "audio" }]
    }"#,
    );
    rig.render(2);
    write(&rig, "clock", "bpm", 45_000.0.into());
    write(&rig, "clock", "gate_length", 2.0.into());
    assert_eq!(read(&rig, "clock", "bpm"), 22_500.0.into(), "pending");

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(read(&rig, "clock", "bpm"), 45_000.0.into());
    assert_eq!(read(&rig, "clock", "gate_length"), 1.0.into(), "clamped");
    write(&rig, "clock", "gate_length", 0.5.into());
    rig.render(1);
    // Beats now come every 64 samples, high for the first 32 of each.
    let gate = rig.render(2);
    let rises: Vec<usize> = (1..gate.len())
        .filter(|&i| gate[i] > 0.0 && gate[i - 1] == 0.0)
        .collect();
    assert!(rises.len() >= 2, "{rises:?}");
    assert!(
        rises.windows(2).all(|pair| pair[1] - pair[0] == 64),
        "{rises:?}"
    );
}
