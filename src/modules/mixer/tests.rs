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

    mixer.set_input("audio.0", 0.5).unwrap();
    mixer.set_input("audio.1", 0.3).unwrap();
    mixer.process(1);

    let left = mixer.get_output("audio_left").unwrap();
    let right = mixer.get_output("audio_right").unwrap();

    approx_eq(left, 0.8 * 2.0_f32.sqrt().recip());
    approx_eq(right, 0.8 * 2.0_f32.sqrt().recip());
}

#[test]
fn test_mixer_with_levels() {
    let mut mixer = Mixer::new(2)
        .with_level(0, 0.5) // Channel 1 at 50%
        .with_level(1, 1.0); // Channel 2 at 100%

    mixer.set_input("audio.0", 1.0).unwrap();
    mixer.set_input("audio.1", 1.0).unwrap();
    mixer.process(1);

    let left = mixer.get_output("audio_left").unwrap();
    let right = mixer.get_output("audio_right").unwrap();
    let expected = 1.5 * 2.0_f32.sqrt().recip();

    approx_eq(left, expected);
    approx_eq(right, expected);
}

#[test]
fn test_mixer_master_level() {
    let mut mixer = Mixer::new(2).with_master(0.5);

    mixer.set_input("audio.0", 1.0).unwrap();
    mixer.set_input("audio.1", 1.0).unwrap();
    mixer.process(1);

    let left = mixer.get_output("audio_left").unwrap();
    let right = mixer.get_output("audio_right").unwrap();
    let expected = 2.0_f32.sqrt().recip();

    approx_eq(left, expected);
    approx_eq(right, expected);
}

#[test]
fn test_mixer_level_cv() {
    let mut mixer = Mixer::new(2);

    mixer.set_input("audio.0", 1.0).unwrap();
    mixer.set_input("audio.1", 1.0).unwrap();
    mixer.set_input("level.0", 0.5).unwrap(); // CV reduces channel 1
    mixer.set_input("level.1", 1.0).unwrap();
    mixer.process(1);

    let left = mixer.get_output("audio_left").unwrap();
    let right = mixer.get_output("audio_right").unwrap();
    let expected = 1.5 * 2.0_f32.sqrt().recip();

    approx_eq(left, expected);
    approx_eq(right, expected);
}

#[test]
fn test_mixer_master_cv() {
    let mut mixer = Mixer::new(2);

    mixer.set_input("audio.0", 1.0).unwrap();
    mixer.set_input("audio.1", 1.0).unwrap();
    mixer.set_input("master", 0.25).unwrap();
    mixer.process(1);

    let left = mixer.get_output("audio_left").unwrap();
    let right = mixer.get_output("audio_right").unwrap();
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
    assert!(inputs.contains(&"audio.0"));
    assert!(inputs.contains(&"audio.1"));
    assert!(inputs.contains(&"audio.2"));
    assert!(!inputs.contains(&"audio.3"));

    assert!(inputs.contains(&"level.0"));
    assert!(inputs.contains(&"level.1"));
    assert!(inputs.contains(&"level.2"));
    assert!(!inputs.contains(&"level.3"));

    assert!(inputs.contains(&"pan.0"));
    assert!(inputs.contains(&"pan.1"));
    assert!(inputs.contains(&"pan.2"));
    assert!(!inputs.contains(&"pan.3"));
}

#[test]
fn test_mixer_expanded_channel_ports_and_pan() {
    let mut mixer = Mixer::new(16).with_pan(15, 1.0);
    assert_eq!(mixer.channel_count(), 16);

    let inputs = mixer.inputs();
    assert!(inputs.contains(&"audio.15"));
    assert!(inputs.contains(&"level.15"));
    assert!(inputs.contains(&"pan.15"));
    assert!(!inputs.contains(&"audio.16"));

    mixer.set_input("audio.15", 1.0).unwrap();
    mixer.process(1);

    approx_eq(mixer.get_output("audio_left").unwrap(), 0.0);
    approx_eq(mixer.get_output("audio_right").unwrap(), 1.0);
}

#[test]
fn test_mixer_invalid_port() {
    let mut mixer = Mixer::new(2);

    // audio.2 doesn't exist on 2-channel mixer
    assert!(mixer.set_input("audio.2", 1.0).is_err());
    assert!(mixer.set_input("level.2", 1.0).is_err());
    assert!(mixer.set_input("pan.2", 1.0).is_err());
    assert!(mixer.get_output("invalid").is_err());
}

#[test]
fn test_mixer_factory() {
    let factory = MixerFactory;
    assert_eq!(ModuleFactory::type_id(&factory), "mixer");

    let config = serde_json::json!({
        "channel_count": 3,
        "levels": [0.8, 0.6, 0.4],
        "pans": [-1.0, 0.0, 1.0],
        "master": 0.9
    });

    let result = factory.build(44100, &config).unwrap();

    let module = result.module.module();
    assert_eq!(module.name(), "Mixer");

    // Should have audio.0-2, level.0-2, pan.0-2, and master
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

    mixer.set_input("audio.0", 0.5).unwrap();
    mixer.set_input("audio.1", -0.5).unwrap();
    mixer.process(1);

    let left = mixer.get_output("audio_left").unwrap();
    let right = mixer.get_output("audio_right").unwrap();
    assert!(left.abs() < 0.001, "Expected ~0 left, got {}", left);
    assert!(right.abs() < 0.001, "Expected ~0 right, got {}", right);
}

#[test]
fn test_mixer_hard_panning() {
    let mut mixer = Mixer::new(2).with_pan(0, -1.0).with_pan(1, 1.0);

    mixer.set_input("audio.0", 1.0).unwrap();
    mixer.set_input("audio.1", 1.0).unwrap();
    mixer.process(1);

    approx_eq(mixer.get_output("audio_left").unwrap(), 1.0);
    approx_eq(mixer.get_output("audio_right").unwrap(), 1.0);
}

#[test]
fn test_mixer_pan_modulation_input() {
    let mut mixer = Mixer::new(1);

    mixer.set_input("audio.0", 1.0).unwrap();
    mixer.set_input("pan.0", 1.0).unwrap();
    mixer.process(1);

    approx_eq(mixer.get_output("audio_left").unwrap(), 0.0);
    approx_eq(mixer.get_output("audio_right").unwrap(), 1.0);
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

#[test]
fn a_whole_float_channel_count_builds_that_many_channels() {
    let built = MixerFactory
        .build(44_100, &serde_json::json!({ "channel_count": 2.0 }))
        .unwrap();
    // audio.0-1, level.0-1, pan.0-1 and master.
    assert_eq!(built.module.module().inputs().len(), 7);
    let error = MixerFactory
        .build(44_100, &serde_json::json!({ "channel_count": 2.5 }))
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.starts_with("mixer config 'channel_count' expects a whole number"),
        "{error}"
    );
}

#[test]
fn levels_and_pans_refuse_a_non_number_rather_than_drop_it() {
    // Dropping `"0.2"` used to shift 0.4 onto the second channel.
    for (config, refusal) in [
        (
            serde_json::json!({ "levels": [0.8, "0.2", 0.4] }),
            "mixer config 'levels[1]' expects a finite number, got \"0.2\"",
        ),
        (
            serde_json::json!({ "pans": [-0.5, null] }),
            "mixer config 'pans[1]' expects a finite number, got null",
        ),
        (
            serde_json::json!({ "pans": [1e39] }),
            "mixer config 'pans[0]' expects a finite number, got 1e39",
        ),
        (
            serde_json::json!({ "levels": 0.5 }),
            "mixer config 'levels' expects an array of numbers, got 0.5",
        ),
    ] {
        let error = MixerFactory
            .build(44_100, &config)
            .err()
            .map(|e| e.to_string());
        assert_eq!(error.as_deref(), Some(refusal), "{config}");
    }
}

#[test]
fn ports_count_from_zero_and_the_old_names_are_gone() {
    let mut mixer = Mixer::new(2);
    assert_eq!(
        mixer.inputs(),
        ["audio.0", "audio.1", "level.0", "level.1", "pan.0", "pan.1", "master"]
    );
    assert_eq!(mixer.outputs(), ["audio_left", "audio_right"]);
    for old in [
        "in1", "in2", "level1", "pan1", "audio.2", "audio.01", "audio.+1",
    ] {
        assert!(mixer.set_input(old, 1.0).is_err(), "{old}");
    }
    assert!(mixer.get_output("left").is_err());
    assert!(mixer.get_output("right").is_err());
    // `level.0` drives the channel the control `level.0` sets.
    mixer.set_input("audio.0", 1.0).unwrap();
    mixer.set_input("level.0", 0.0).unwrap();
    mixer.process(1);
    approx_eq(mixer.get_output("audio_left").unwrap(), 0.0);
}

/// Plays `frames` single frames of a constant 1.0 on channel 0 and returns
/// the left output of each.
fn left_outputs(mixer: &mut Mixer, frames: u32) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            mixer.set_input("audio.0", 1.0).unwrap();
            mixer.process(1);
            mixer.get_output("audio_left").unwrap()
        })
        .collect()
}

/// How far one glide step may stray from an even one: f32 rounding gathered
/// over a glide, far below the step a click would be.
const STEP_TOLERANCE: f32 = 0.05;
/// How close a glide's last frame, or its midpoint, is to where it should be.
const LANDING_TOLERANCE: f32 = 1e-6;
const MIDPOINT_TOLERANCE: f32 = 1e-5;
/// A block of the size a live device plays.
pub(super) const BLOCK: usize = 128;

/// The glide of a mixer built without a sample rate.
pub(super) fn ramp_frames() -> u32 {
    gain_ramp_frames(DEFAULT_SAMPLE_RATE)
}

/// Whether `outputs` move from `from` to `to` in equal steps and land on it.
fn glides(outputs: &[f32], from: f32, to: f32) -> bool {
    let step = (to - from) / ramp_frames() as f32;
    let mut previous = from;
    let even = outputs.iter().all(|&v| {
        let ok = ((v - previous) - step).abs() <= step.abs() * STEP_TOLERANCE;
        previous = v;
        ok
    });
    even && (outputs.last().unwrap() - to).abs() < LANDING_TOLERANCE
}

#[test]
fn a_master_change_glides_instead_of_stepping() {
    let mut mixer = Mixer::new(1);
    let unity = left_outputs(&mut mixer, 1)[0];
    mixer.set_master(0.0);
    let outputs = left_outputs(&mut mixer, ramp_frames());
    assert!(glides(&outputs, unity, 0.0), "{outputs:?}");
    assert_eq!(mixer.get_control("master").unwrap(), 0.0);
}

#[test]
fn a_level_change_glides_instead_of_stepping() {
    let mut mixer = Mixer::new(1).with_level(0, 0.5);
    let half = left_outputs(&mut mixer, 1)[0];
    mixer.set_level(0, 1.5);
    let outputs = left_outputs(&mut mixer, ramp_frames());
    assert!(glides(&outputs, half, half * 3.0), "{outputs:?}");
}

#[test]
fn a_change_mid_glide_turns_back_without_a_jump() {
    let mut mixer = Mixer::new(1);
    let unity = left_outputs(&mut mixer, 1)[0];
    mixer.set_master(0.0);
    let down = left_outputs(&mut mixer, ramp_frames() / 2);
    let turned_at = *down.last().unwrap();
    mixer.set_master(1.0);
    let back = left_outputs(&mut mixer, ramp_frames());
    assert!(
        (turned_at - unity / 2.0).abs() < MIDPOINT_TOLERANCE,
        "{turned_at}"
    );
    assert!(glides(&back, turned_at, unity), "{back:?}");
}

#[test]
fn the_first_block_starts_at_the_controls_values() {
    let mut mixer = Mixer::new(1).with_master(0.25);
    let first = left_outputs(&mut mixer, 1)[0];
    assert!((first - 0.25 * 2.0_f32.sqrt().recip()).abs() < LANDING_TOLERANCE);
}

#[test]
fn the_glide_lasts_the_same_time_at_any_sample_rate() {
    assert_eq!(gain_ramp_frames(48_000), 480);
    assert_eq!(gain_ramp_frames(44_100), 441);
    assert_eq!(gain_ramp_frames(96_000), 960);
}

/// Plays `blocks` blocks of BLOCK frames of a constant 1.0 on channel 0 and
/// returns the left output of every frame.
pub(super) fn left_blocks(mixer: &mut dyn Module, blocks: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..blocks {
        mixer.input_block_mut(0)[..BLOCK].fill(1.0);
        mixer.process(BLOCK);
        out.extend_from_slice(&mixer.output_block(0)[..BLOCK]);
    }
    out
}

#[test]
fn a_glide_runs_on_across_blocks() {
    let mut mixer = Mixer::new(1);
    let unity = left_blocks(&mut mixer, 1)[0];
    mixer.set_master(0.0);
    let frames = ramp_frames() as usize;
    let outputs = left_blocks(&mut mixer, frames.div_ceil(BLOCK) + 1);
    assert!(glides(&outputs[..frames], unity, 0.0), "{outputs:?}");
    assert!(outputs[frames..].iter().all(|&v| v == 0.0));
}

#[test]
fn the_factory_times_the_glide_from_the_sample_rate() {
    const RATE: u32 = 44_100;
    let config = serde_json::json!({ "channel_count": 1 });
    let mut built = MixerFactory.build(RATE, &config).unwrap();
    let mixer = built.module.module_mut();
    left_blocks(mixer, 1);
    mixer.set_control("master", 0.0).unwrap();
    let outputs = left_blocks(mixer, 4);
    let frames = gain_ramp_frames(RATE) as usize;
    assert!(outputs[frames - 2] > 0.0, "{outputs:?}");
    assert_eq!(outputs[frames - 1], 0.0);
}

#[test]
fn a_glide_never_allocates() {
    let mut mixer = Mixer::new(2);
    let blocks = (ramp_frames() as usize).div_ceil(BLOCK) + 1;
    mixer.process(BLOCK);
    let ((), allocs, frees) = crate::alloc_counter::allocator_events(|| {
        mixer.set_master(0.25);
        mixer.set_level(1, 0.5);
        mixer.set_pan(0, -0.5);
        mixer.set_input("pan.1", 0.3).unwrap();
        for _ in 0..blocks {
            mixer.input_block_mut(0)[..BLOCK].fill(1.0);
            mixer.input_block_mut(1)[..BLOCK].fill(1.0);
            mixer.process(BLOCK);
        }
    });
    assert_eq!((allocs, frees), (0, 0));
}
