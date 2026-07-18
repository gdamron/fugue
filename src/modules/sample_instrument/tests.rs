use super::*;
use crate::factory::ModuleFactory;
use crate::{ControlSurface, ControlValue};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Process-unique suffix so parallel tests never collide on a temp path.
/// (The shared sample cache is keyed by path, so a reused path would
/// otherwise serve a stale buffer.)
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}", nanos, seq)
}

fn temp_wav_path() -> PathBuf {
    std::env::temp_dir().join(format!("fugue-sample-instrument-{}.wav", unique_suffix()))
}

/// A mono WAV at 44.1k holding exactly `levels` as its frames.
fn write_levels_wav(levels: &[f32]) -> PathBuf {
    let path = temp_wav_path();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for level in levels {
        writer
            .write_sample((level.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
    path
}

/// A mono WAV whose every frame has amplitude `level`.
fn write_level_wav(level: f32, frames: usize) -> PathBuf {
    write_levels_wav(&vec![level; frames])
}

fn build_instrument(
    config: serde_json::Value,
) -> (Box<dyn crate::Module + Send>, SampleInstrumentControls) {
    let result = SampleInstrumentFactory.build(44_100, &config).unwrap();
    let controls = result
        .handles
        .iter()
        .find(|(name, _)| name == "controls")
        .unwrap()
        .1
        .downcast_ref::<SampleInstrumentControls>()
        .unwrap()
        .clone();
    let module = match result.module {
        crate::factory::GraphModule::Module(module) => module,
        crate::factory::GraphModule::Sink(_) => panic!("sample_instrument is not a sink"),
    };
    (module, controls)
}

fn freq(note: u8) -> f32 {
    crate::music::Note::new(note).frequency()
}

/// Runs `frames` frames one block-of-one at a time, so `get_output` (which
/// reads frame 0 of the last block) reports the most recent frame.
fn run(instrument: &mut Box<dyn crate::Module + Send>, frames: usize) {
    for _ in 0..frames {
        instrument.process(1);
    }
}

/// 16-bit WAV quantization tolerance.
const TOL: f32 = 2e-3;

#[test]
fn test_gate_starts_note_at_zone_root() {
    let path = write_level_wav(0.5, 8);
    let config = serde_json::json!({
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, _controls) = build_instrument(config);

    // No gate yet: silence.
    instrument.process(1);
    assert_eq!(instrument.get_output("audio_left").unwrap(), 0.0);

    instrument.set_input("frequency", freq(60)).unwrap();
    instrument.set_input("gate", 1.0).unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.5).abs() < TOL);
    assert!((instrument.get_output("audio_right").unwrap() - 0.5).abs() < TOL);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_note_resolves_to_containing_zone() {
    let low = write_level_wav(0.2, 8);
    let high = write_level_wav(0.4, 8);
    let config = serde_json::json!({
        "zones": [
            { "root": 48, "key_range": [36, 59], "asset": low.to_str().unwrap() },
            { "root": 72, "key_range": [60, 84], "asset": high.to_str().unwrap() }
        ]
    });
    let (mut instrument, _controls) = build_instrument(config);

    // Note 62 lands in the second zone's range (constant sample, so the
    // pitch ratio does not change the level).
    instrument.set_input("frequency", freq(62)).unwrap();
    instrument.set_input("gate", 1.0).unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.4).abs() < TOL);

    let _ = std::fs::remove_file(low);
    let _ = std::fs::remove_file(high);
}

#[test]
fn test_note_outside_all_ranges_resolves_to_nearest_zone() {
    let low = write_level_wav(0.2, 64);
    let high = write_level_wav(0.4, 64);
    let config = serde_json::json!({
        "zones": [
            { "root": 48, "key_range": [40, 56], "asset": low.to_str().unwrap() },
            { "root": 72, "key_range": [64, 80], "asset": high.to_str().unwrap() }
        ]
    });
    let (mut instrument, _controls) = build_instrument(config);

    // Note 30 is below every range: nearest is the low zone.
    instrument.set_input("frequency", freq(30)).unwrap();
    instrument.set_input("gate", 1.0).unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.2).abs() < TOL);

    // Note 61 sits between the ranges, nearer the high zone's edge (64)
    // than the low zone's (56).
    instrument.set_input("gate", 0.0).unwrap();
    instrument.process(1);
    instrument.set_input("frequency", freq(61)).unwrap();
    instrument.set_input("gate", 1.0).unwrap();
    instrument.process(1);
    // Both notes sound (the first is releasing); the new one adds 0.4.
    let level = instrument.get_output("audio_left").unwrap();
    assert!(level > 0.4 - TOL, "{level}");

    let _ = std::fs::remove_file(low);
    let _ = std::fs::remove_file(high);
}

#[test]
fn test_pitch_ratio_derives_from_zone_root() {
    // A rising staircase distinguishes read-head speeds: at an octave above
    // the root the head advances two frames per output frame.
    let path = write_levels_wav(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]);
    let config = serde_json::json!({
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, _controls) = build_instrument(config);

    instrument.set_input("frequency", freq(72)).unwrap();
    instrument.set_input("gate", 1.0).unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.1).abs() < TOL);
    instrument.process(1);
    // One frame later the head sits at source frame ~2.
    assert!((instrument.get_output("audio_left").unwrap() - 0.3).abs() < 2.0 * TOL);
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.5).abs() < 2.0 * TOL);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_gate_fall_releases_with_fade() {
    let path = write_level_wav(0.4, 44_100);
    let config = serde_json::json!({
        // The 1 ms floor: a ~44-frame release fade at 44.1 kHz.
        "release": 0.001,
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, _controls) = build_instrument(config);

    instrument.set_input("frequency", freq(60)).unwrap();
    instrument.set_input("gate", 1.0).unwrap();
    instrument.process(2);

    instrument.set_input("gate", 0.0).unwrap();
    // The release frame still sounds at full level, then fades strictly
    // (no jump to silence) and reaches zero within ~45 frames.
    instrument.process(1);
    let mut last = instrument.get_output("audio_left").unwrap();
    assert!((last - 0.4).abs() < TOL);
    for frame in 0..50 {
        instrument.process(1);
        let level = instrument.get_output("audio_left").unwrap();
        assert!(
            level < last || (level == 0.0 && last == 0.0),
            "fade stalled at frame {frame}: {last} -> {level}"
        );
        assert!(last - level < 0.02, "fade jumped at frame {frame}");
        last = level;
    }
    assert_eq!(last, 0.0);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_sustain_loop_holds_past_sample_end_and_releases_through_tail() {
    // 8 frames: loop frames 2..6 sustain at 0.4; the tail (frames 6, 7)
    // decays, distinguishing loop exit from continued wrapping.
    let path = write_levels_wav(&[0.4, 0.4, 0.4, 0.4, 0.4, 0.4, 0.2, 0.1]);
    let config = serde_json::json!({
        "release": 1.0,
        "zones": [ {
            "root": 60,
            "key_range": [0, 127],
            "asset": path.to_str().unwrap(),
            "loop": { "start_frames": 2, "end_frames": 6 }
        } ]
    });
    let (mut instrument, _controls) = build_instrument(config);

    instrument.set_input("frequency", freq(60)).unwrap();
    instrument.set_input("gate", 1.0).unwrap();
    // Hold far past the 8-frame sample: the loop keeps it sounding.
    instrument.process(64);
    assert!((instrument.get_output("audio_left").unwrap() - 0.4).abs() < TOL);

    // Gate fall: playback exits the loop into the tail. With a 1-second
    // release the fade is negligible over a few frames, so the tail's
    // decaying levels show through.
    instrument.set_input("gate", 0.0).unwrap();
    let mut saw_tail = false;
    for _ in 0..8 {
        instrument.process(1);
        let level = instrument.get_output("audio_left").unwrap();
        if (level - 0.1).abs() < 2.0 * TOL {
            saw_tail = true;
        }
    }
    assert!(saw_tail, "release never reached the sample tail");
    // Past the sample end the voice retires.
    instrument.process(4);
    assert_eq!(instrument.get_output("audio_left").unwrap(), 0.0);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_two_notes_in_same_zone_do_not_choke() {
    let path = write_level_wav(0.3, 44_100);
    let config = serde_json::json!({
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, controls) = build_instrument(config);

    // Two held notes resolving to the same zone, via control-thread events.
    controls
        .set_control("note_on", ControlValue::Number(60.0))
        .unwrap();
    controls
        .set_control("note_on", ControlValue::Number(64.0))
        .unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.6).abs() < 2.0 * TOL);

    // Releasing one leaves the other sustaining: after the default 0.1 s
    // release has run out, exactly one voice's level remains.
    controls
        .set_control("note_off", ControlValue::Number(60.0))
        .unwrap();
    run(&mut instrument, 44_100 / 5);
    assert!((instrument.get_output("audio_left").unwrap() - 0.3).abs() < 2.0 * TOL);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_retrigger_same_note_reuses_its_voice() {
    // A sample that opens loud then holds quiet distinguishes a restart from
    // a continuation.
    let mut levels = vec![0.5];
    levels.extend(std::iter::repeat_n(0.1, 2000));
    let path = write_levels_wav(&levels);
    let config = serde_json::json!({
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, controls) = build_instrument(config);

    controls
        .set_control("note_on", ControlValue::Number(60.0))
        .unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.5).abs() < TOL);
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.1).abs() < TOL);

    controls
        .set_control("note_on", ControlValue::Number(60.0))
        .unwrap();
    instrument.process(1);
    // Restarted from the top of the sample on the same voice. The taken-over
    // note's 0.1 tail rides along under the declick ramp, so the frame is
    // the restart plus that tail — not two full voices.
    assert!((instrument.get_output("audio_left").unwrap() - 0.6).abs() < 2.0 * TOL);

    // Once the ramp has run out only the restarted note remains.
    run(&mut instrument, 300);
    assert!((instrument.get_output("audio_left").unwrap() - 0.1).abs() < TOL);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_gate_fall_releases_held_note_after_frequency_moves() {
    // A sequencer may move `frequency` to the next note before the current
    // note's gate falls; the falling edge must still release the sounding
    // voice rather than leaving it stuck on forever.
    let path = write_level_wav(0.4, 44_100);
    let config = serde_json::json!({
        "release": 0.001,
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, _controls) = build_instrument(config);

    instrument.set_input("frequency", freq(60)).unwrap();
    instrument.set_input("gate", 1.0).unwrap();
    instrument.process(2);
    assert!((instrument.get_output("audio_left").unwrap() - 0.4).abs() < TOL);

    instrument.set_input("frequency", freq(67)).unwrap();
    instrument.set_input("gate", 0.0).unwrap();
    run(&mut instrument, 64);
    assert_eq!(instrument.get_output("audio_left").unwrap(), 0.0);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_note_off_control_only_releases_its_own_note() {
    // The explicit control is strictly keyed: releasing a note the pool
    // already stole must not cut an unrelated held note.
    let path = write_level_wav(0.4, 44_100);
    let config = serde_json::json!({
        "release": 0.001,
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, controls) = build_instrument(config);

    controls
        .set_control("note_on", ControlValue::Number(60.0))
        .unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.4).abs() < TOL);

    controls
        .set_control("note_off", ControlValue::Number(64.0))
        .unwrap();
    run(&mut instrument, 64);
    // Note 60 is still held.
    assert!((instrument.get_output("audio_left").unwrap() - 0.4).abs() < TOL);

    controls
        .set_control("note_off", ControlValue::Number(60.0))
        .unwrap();
    run(&mut instrument, 64);
    assert_eq!(instrument.get_output("audio_left").unwrap(), 0.0);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_velocity_scales_note_level() {
    let path = write_level_wav(0.5, 8);
    let config = serde_json::json!({
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, _controls) = build_instrument(config);

    instrument.set_input("frequency", freq(60)).unwrap();
    instrument.set_input("velocity", 0.5).unwrap();
    instrument.set_input("gate", 1.0).unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.25).abs() < TOL);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_zone_gain_control_scales_level() {
    let path = write_level_wav(0.5, 8);
    let config = serde_json::json!({
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap(), "gain": 0.5 } ]
    });
    let (mut instrument, controls) = build_instrument(config);

    instrument.set_input("frequency", freq(60)).unwrap();
    instrument.set_input("gate", 1.0).unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.25).abs() < TOL);

    controls
        .set_control("gain.0", ControlValue::Number(1.0))
        .unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.5).abs() < TOL);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_asset_swap_keeps_sounding_voice_on_old_buffer() {
    let original = write_level_wav(0.5, 44_100);
    let replacement = write_level_wav(0.25, 44_100);
    let config = serde_json::json!({
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": original.to_str().unwrap() } ]
    });
    let (mut instrument, controls) = build_instrument(config);

    controls
        .set_control("note_on", ControlValue::Number(60.0))
        .unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.5).abs() < TOL);

    controls
        .set_control(
            "asset.0",
            ControlValue::String(replacement.to_str().unwrap().to_string()),
        )
        .unwrap();
    assert_eq!(
        controls.get_control("asset.0").unwrap(),
        ControlValue::String(replacement.to_str().unwrap().to_string())
    );
    // The sounding voice latched the old buffer at note-on.
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.5).abs() < TOL);

    // A different note starts on the new buffer; both sound together.
    controls
        .set_control("note_on", ControlValue::Number(64.0))
        .unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.75).abs() < 2.0 * TOL);

    let _ = std::fs::remove_file(original);
    let _ = std::fs::remove_file(replacement);
}

#[test]
fn test_voice_pool_steals_oldest_when_exhausted() {
    let path = write_level_wav(0.2, 44_100);
    let config = serde_json::json!({
        "voices": 2,
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, controls) = build_instrument(config);

    for note in [60.0, 64.0, 67.0] {
        controls
            .set_control("note_on", ControlValue::Number(note))
            .unwrap();
    }
    instrument.process(1);
    // Three notes into a 2-voice pool: the oldest was stolen. Two notes
    // sound, plus the stolen note's declick tail on the frame it was taken.
    assert!((instrument.get_output("audio_left").unwrap() - 0.6).abs() < 3.0 * TOL);

    // After the ramp only the two live notes remain.
    run(&mut instrument, 300);
    assert!((instrument.get_output("audio_left").unwrap() - 0.4).abs() < 2.0 * TOL);

    let _ = std::fs::remove_file(path);
}

#[test]
fn test_null_config_builds_empty_instrument() {
    // Module type discovery constructs every registered type with a null
    // config, so an empty instrument must build and describe itself.
    let result = SampleInstrumentFactory
        .build(44_100, &serde_json::Value::Null)
        .unwrap();
    let surface = result.control_surface.unwrap();
    let keys: Vec<String> = surface.controls().into_iter().map(|meta| meta.key).collect();
    assert_eq!(keys, ["release", "note_on", "note_off"]);
}

#[test]
fn test_stealing_a_sounding_voice_does_not_step_the_output() {
    // Pool exhaustion must ramp the taken-over note out, not cut it: a hard
    // cut mid-waveform is an audible click. The sample opens with an attack
    // (as a real instrument recording does), so a stealing note contributes
    // ~0 on its first frame and the only thing that could step the mix is
    // the disappearance of the note being taken over.
    const ATTACK: usize = 200;
    let mut levels: Vec<f32> = (0..ATTACK)
        .map(|i| 0.4 * i as f32 / ATTACK as f32)
        .collect();
    levels.extend(std::iter::repeat_n(0.4, 44_100));
    let path = write_levels_wav(&levels);
    let config = serde_json::json!({
        "voices": 1,
        "release": 5.0,
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (mut instrument, controls) = build_instrument(config);

    controls
        .set_control("note_on", ControlValue::Number(60.0))
        .unwrap();
    run(&mut instrument, ATTACK + 64);
    let before = instrument.get_output("audio_left").unwrap();
    assert!((before - 0.4).abs() < TOL, "{before}");

    // A different note steals the only voice while the first is sounding.
    controls
        .set_control("note_on", ControlValue::Number(67.0))
        .unwrap();
    instrument.process(1);
    let after = instrument.get_output("audio_left").unwrap();
    assert!(
        (after - before).abs() < 0.05,
        "steal stepped the output: {before} -> {after}"
    );

    // Every frame across the take-over stays continuous, not just the first.
    for frame in 0..ATTACK {
        instrument.process(1);
        let level = instrument.get_output("audio_left").unwrap();
        assert!(
            (level - before).abs() < 0.08,
            "steal stepped at frame {frame}: {before} -> {level}"
        );
    }

    // The ramp is short: the stolen note's tail is gone well under 10 ms,
    // leaving the new note alone at its own level.
    run(&mut instrument, 441);
    assert!((instrument.get_output("audio_left").unwrap() - 0.4).abs() < TOL);

    let _ = std::fs::remove_file(path);
}

#[test]
fn registry_builds_sample_instrument_with_its_ports() {
    let registry = crate::ModuleRegistry::default();
    assert!(registry.has_type("sample_instrument"));

    // Type discovery builds every registered type with a null config.
    let built = registry
        .build("sample_instrument", 44_100, &serde_json::Value::Null)
        .unwrap();
    let module = built.module.module();
    assert_eq!(module.inputs(), inputs::INPUTS.as_slice());
    assert_eq!(module.outputs(), outputs::OUTPUTS.as_slice());
}

#[test]
fn test_config_validation_errors() {
    let wav = write_level_wav(0.5, 8);
    let wav_str = wav.to_str().unwrap();

    let cases = [
        (
            serde_json::json!({ "zones": [{ "asset": wav_str }] }),
            "'root' must be a MIDI note number",
        ),
        (
            serde_json::json!({ "zones": [{ "root": 200, "asset": wav_str }] }),
            "'root' must be a MIDI note number",
        ),
        (
            serde_json::json!({ "zones": [{ "root": 60 }] }),
            "missing 'asset'",
        ),
        (
            serde_json::json!({ "zones": [{ "root": 60, "key_range": [60], "asset": wav_str }] }),
            "'key_range' must be [low, high]",
        ),
        (
            serde_json::json!({ "zones": [{ "root": 60, "key_range": [70, 60], "asset": wav_str }] }),
            "low exceeds high",
        ),
        (
            serde_json::json!({ "zones": [{ "root": 60, "asset": wav_str, "gain": "loud" }] }),
            "'gain' must be a number",
        ),
        (
            serde_json::json!({ "zones": [{ "root": 60, "asset": wav_str,
                "loop": { "start_frames": 6, "end_frames": 2 } }] }),
            "end_frames must be greater than start_frames",
        ),
        (
            serde_json::json!({ "zones": [{ "root": 60, "asset": wav_str,
                "loop": { "start_frames": 2, "end_frames": 400 } }] }),
            "past the end of the sample",
        ),
        (
            serde_json::json!({ "zones": [{ "root": 60, "asset": wav_str,
                "loop": { "start_frames": 2, "end_frames": 6, "crossfade_frames": 4 } }] }),
            "crossfade_frames must not exceed start_frames",
        ),
        (
            serde_json::json!({ "zones": {} }),
            "'zones' must be an array",
        ),
        (
            serde_json::json!({ "voices": 0 }),
            "'voices' must be an integer",
        ),
        (
            serde_json::json!({ "voices": 99 }),
            "'voices' must be an integer",
        ),
        (
            serde_json::json!({ "release": -1.0 }),
            "'release' must be a positive number",
        ),
    ];

    for (config, expected) in cases {
        let err = SampleInstrumentFactory
            .build(44_100, &config)
            .err()
            .unwrap_or_else(|| panic!("expected error for {config}"))
            .to_string();
        assert!(err.contains(expected), "{config}: {err}");
    }

    let _ = std::fs::remove_file(wav);
}

#[test]
fn test_note_control_validation() {
    let path = write_level_wav(0.5, 8);
    let config = serde_json::json!({
        "zones": [ { "root": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let (_instrument, controls) = build_instrument(config);

    let err = controls
        .set_control("note_on", ControlValue::Number(200.0))
        .unwrap_err();
    assert!(err.contains("outside MIDI range"), "{err}");
    let err = controls
        .set_control("note_on", ControlValue::String("loud".to_string()))
        .unwrap_err();
    assert!(err.contains("expected a MIDI note number"), "{err}");
    // Numeric strings are accepted.
    controls
        .set_control("note_on", ControlValue::String("60".to_string()))
        .unwrap();

    let _ = std::fs::remove_file(path);
}
