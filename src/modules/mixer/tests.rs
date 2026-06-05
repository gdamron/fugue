use super::*;

fn approx_eq(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.001,
        "Expected {}, got {}",
        expected,
        actual
    );
}

#[test]
fn test_mixer_basic_summing() {
    let mut mixer = Mixer::new(2);

    mixer.set_input("in1", 0.5).unwrap();
    mixer.set_input("in2", 0.3).unwrap();
    mixer.process(1);

    let left = mixer.get_output("left").unwrap();
    let right = mixer.get_output("right").unwrap();

    approx_eq(left, 0.8 * 2.0_f32.sqrt().recip());
    approx_eq(right, 0.8 * 2.0_f32.sqrt().recip());
}

#[test]
fn test_mixer_with_levels() {
    let mut mixer = Mixer::new(2)
        .with_level(0, 0.5) // Channel 1 at 50%
        .with_level(1, 1.0); // Channel 2 at 100%

    mixer.set_input("in1", 1.0).unwrap();
    mixer.set_input("in2", 1.0).unwrap();
    mixer.process(1);

    let left = mixer.get_output("left").unwrap();
    let right = mixer.get_output("right").unwrap();
    let expected = 1.5 * 2.0_f32.sqrt().recip();

    approx_eq(left, expected);
    approx_eq(right, expected);
}

#[test]
fn test_mixer_master_level() {
    let mut mixer = Mixer::new(2).with_master(0.5);

    mixer.set_input("in1", 1.0).unwrap();
    mixer.set_input("in2", 1.0).unwrap();
    mixer.process(1);

    let left = mixer.get_output("left").unwrap();
    let right = mixer.get_output("right").unwrap();
    let expected = 2.0_f32.sqrt().recip();

    approx_eq(left, expected);
    approx_eq(right, expected);
}

#[test]
fn test_mixer_level_cv() {
    let mut mixer = Mixer::new(2);

    mixer.set_input("in1", 1.0).unwrap();
    mixer.set_input("in2", 1.0).unwrap();
    mixer.set_input("level1", 0.5).unwrap(); // CV reduces channel 1
    mixer.set_input("level2", 1.0).unwrap();
    mixer.process(1);

    let left = mixer.get_output("left").unwrap();
    let right = mixer.get_output("right").unwrap();
    let expected = 1.5 * 2.0_f32.sqrt().recip();

    approx_eq(left, expected);
    approx_eq(right, expected);
}

#[test]
fn test_mixer_master_cv() {
    let mut mixer = Mixer::new(2);

    mixer.set_input("in1", 1.0).unwrap();
    mixer.set_input("in2", 1.0).unwrap();
    mixer.set_input("master", 0.25).unwrap();
    mixer.process(1);

    let left = mixer.get_output("left").unwrap();
    let right = mixer.get_output("right").unwrap();
    let expected = 0.5 * 2.0_f32.sqrt().recip();

    approx_eq(left, expected);
    approx_eq(right, expected);
}

#[test]
fn test_mixer_channel_count() {
    let mixer = Mixer::new(3);
    assert_eq!(mixer.channel_count(), 3);

    // Check that only 3 input channels exist
    let inputs = mixer.inputs();
    assert!(inputs.contains(&"in1"));
    assert!(inputs.contains(&"in2"));
    assert!(inputs.contains(&"in3"));
    assert!(!inputs.contains(&"in4"));

    assert!(inputs.contains(&"level1"));
    assert!(inputs.contains(&"level2"));
    assert!(inputs.contains(&"level3"));
    assert!(!inputs.contains(&"level4"));

    assert!(inputs.contains(&"pan1"));
    assert!(inputs.contains(&"pan2"));
    assert!(inputs.contains(&"pan3"));
    assert!(!inputs.contains(&"pan4"));
}

#[test]
fn test_mixer_expanded_channel_ports_and_pan() {
    let mut mixer = Mixer::new(16).with_pan(15, 1.0);
    assert_eq!(mixer.channel_count(), 16);

    let inputs = mixer.inputs();
    assert!(inputs.contains(&"in16"));
    assert!(inputs.contains(&"level16"));
    assert!(inputs.contains(&"pan16"));
    assert!(!inputs.contains(&"in17"));

    mixer.set_input("in16", 1.0).unwrap();
    mixer.process(1);

    approx_eq(mixer.get_output("left").unwrap(), 0.0);
    approx_eq(mixer.get_output("right").unwrap(), 1.0);
}

#[test]
fn test_mixer_invalid_port() {
    let mut mixer = Mixer::new(2);

    // in3 doesn't exist on 2-channel mixer
    assert!(mixer.set_input("in3", 1.0).is_err());
    assert!(mixer.set_input("level3", 1.0).is_err());
    assert!(mixer.set_input("pan3", 1.0).is_err());
    assert!(mixer.get_output("invalid").is_err());
}

#[test]
fn test_mixer_factory() {
    let factory = MixerFactory;
    assert_eq!(ModuleFactory::type_id(&factory), "mixer");

    let config = serde_json::json!({
        "channels": 3,
        "levels": [0.8, 0.6, 0.4],
        "pans": [-1.0, 0.0, 1.0],
        "master": 0.9
    });

    let result = factory.build(44100, &config).unwrap();

    let module = result.module.module();
    assert_eq!(module.name(), "Mixer");

    // Should have in1-in3, level1-level3, pan1-pan3, and master
    let inputs = module.inputs();
    assert_eq!(inputs.len(), 10); // 3 ins + 3 levels + 3 pans + 1 master
}

#[test]
fn test_mixer_clamps_channels() {
    // Too few
    let mixer = Mixer::new(0);
    assert_eq!(mixer.channel_count(), 1);

    // Too many
    let mixer = Mixer::new(100);
    assert_eq!(mixer.channel_count(), MAX_CHANNELS);
}

#[test]
fn test_mixer_negative_input() {
    let mut mixer = Mixer::new(2);

    mixer.set_input("in1", 0.5).unwrap();
    mixer.set_input("in2", -0.5).unwrap();
    mixer.process(1);

    let left = mixer.get_output("left").unwrap();
    let right = mixer.get_output("right").unwrap();
    assert!(left.abs() < 0.001, "Expected ~0 left, got {}", left);
    assert!(right.abs() < 0.001, "Expected ~0 right, got {}", right);
}

#[test]
fn test_mixer_hard_panning() {
    let mut mixer = Mixer::new(2).with_pan(0, -1.0).with_pan(1, 1.0);

    mixer.set_input("in1", 1.0).unwrap();
    mixer.set_input("in2", 1.0).unwrap();
    mixer.process(1);

    approx_eq(mixer.get_output("left").unwrap(), 1.0);
    approx_eq(mixer.get_output("right").unwrap(), 1.0);
}

#[test]
fn test_mixer_pan_modulation_input() {
    let mut mixer = Mixer::new(1);

    mixer.set_input("in1", 1.0).unwrap();
    mixer.set_input("pan1", 1.0).unwrap();
    mixer.process(1);

    approx_eq(mixer.get_output("left").unwrap(), 0.0);
    approx_eq(mixer.get_output("right").unwrap(), 1.0);
}

#[test]
fn test_mixer_controls() {
    let mut mixer = Mixer::new(2);

    // Test control metadata
    let control_meta = Module::controls(&mixer);
    assert_eq!(control_meta.len(), 5); // level.0, pan.0, level.1, pan.1, master
    assert_eq!(control_meta[0].key, "level.0");
    assert_eq!(control_meta[1].key, "pan.0");
    assert_eq!(control_meta[2].key, "level.1");
    assert_eq!(control_meta[3].key, "pan.1");
    assert_eq!(control_meta[4].key, "master");

    // Test get/set controls
    mixer.set_control("level.0", 0.5).unwrap();
    assert_eq!(mixer.get_control("level.0").unwrap(), 0.5);

    mixer.set_control("pan.0", -0.25).unwrap();
    assert_eq!(mixer.get_control("pan.0").unwrap(), -0.25);

    mixer.set_control("master", 0.8).unwrap();
    assert_eq!(mixer.get_control("master").unwrap(), 0.8);

    // Test invalid control
    assert!(mixer.get_control("invalid").is_err());
    assert!(mixer.get_control("level.5").is_err()); // Only 2 channels
    assert!(mixer.get_control("pan.5").is_err()); // Only 2 channels
}
