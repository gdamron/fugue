//! A sample swap and a play or note at the same instant: whichever was
//! written first, the play or note takes the new sample, live and offline
//! alike. (Live, the swap applies first, as a value at one sample does;
//! offline each write applies as it is made, so the module reads the sample
//! as the play or note sounds.)

use super::*;
use crate::{ControlValue, RenderEngine};

/// Writes a mono WAV at the engine rate, a second of `level`, into `dir`.
fn level_wav(dir: &std::path::Path, name: &str, level: f32) -> String {
    let path = dir.join(name);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for _ in 0..SAMPLE_RATE {
        writer
            .write_sample((level * i16::MAX as f32) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
    path.to_string_lossy().into_owned()
}

/// `module` (id `m`) into the DAC.
fn document(module_type: &str, config: serde_json::Value) -> String {
    serde_json::json!({
        "version": "1.0.0",
        "modules": [
            { "id": "m", "type": module_type, "config": config },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [{ "from": "m", "from_port": "audio_left", "to": "dac", "to_port": "audio" }]
    })
    .to_string()
}

/// Two blocks of the DAC's left output, live, with `writes` made after the
/// first, all for one sample.
fn live(json: &str, writes: &[(&str, ControlValue)]) -> Vec<f32> {
    let mut rig = Rig::new(json);
    let mut heard = rig.render(1);
    let surface = rig.live.control_surfaces.lock().unwrap()["m"].clone();
    for (key, value) in writes {
        surface.set_control(key, value.clone()).unwrap();
    }
    heard.extend(rig.render(1));
    heard
}

/// The same, offline.
fn offline(json: &str, writes: &[(&str, ControlValue)]) -> Vec<f32> {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine
        .load_invention(Invention::from_json(json).unwrap())
        .unwrap();
    let mut block = [0.0f32; 128];
    let mut heard = Vec::new();
    for at in 0..2 {
        if at == 1 {
            for (key, value) in writes {
                engine.set_control("m", key, value.clone()).unwrap();
            }
        }
        engine.render_interleaved(&mut block).unwrap();
        heard.extend(block.iter().step_by(2));
    }
    heard
}

/// Each order of `play` and the swap to `quiet`, live and offline, plays
/// `quiet` from the write's first frame.
fn agree(json: &str, play: (&str, ControlValue), quiet: &str) {
    let swap = ("asset.0", ControlValue::from(quiet));
    for writes in [[swap.clone(), play.clone()], [play.clone(), swap.clone()]] {
        let first = writes[0].0;
        let live = live(json, &writes);
        let offline = offline(json, &writes);
        let (l, o) = (live[64], offline[64]);
        assert!(
            live == offline,
            "{first} first: live {l} and offline {o} differ"
        );
        assert!(
            (live[64] - 0.125).abs() < 2e-3,
            "{first} first: {}",
            live[64]
        );
    }
}

#[test]
fn a_kit_play_at_a_swaps_instant_plays_the_new_sample_live_and_offline() {
    let dir = tempfile::tempdir().unwrap();
    let kick = level_wav(dir.path(), "kick.wav", 0.5);
    let quiet = level_wav(dir.path(), "quiet.wav", 0.125);
    let config = serde_json::json!({ "samples": [{ "key": 36, "asset": kick }] });
    agree(
        &document("sample_kit", config),
        ("play", 36.0.into()),
        &quiet,
    );
}

#[test]
fn an_instrument_note_at_a_swaps_instant_plays_the_new_sample_live_and_offline() {
    let dir = tempfile::tempdir().unwrap();
    let half = level_wav(dir.path(), "half.wav", 0.5);
    let quiet = level_wav(dir.path(), "quiet.wav", 0.125);
    let config = serde_json::json!({
        "zones": [{ "root_note": 60, "key_range": [0, 127], "asset": half }]
    });
    let json = document("sample_instrument", config);
    agree(&json, ("note_on", 60.0.into()), &quiet);
}

#[test]
fn a_swap_after_a_play_has_sounded_waits_for_the_next_play() {
    let dir = tempfile::tempdir().unwrap();
    let kick = level_wav(dir.path(), "kick.wav", 0.5);
    let quiet = level_wav(dir.path(), "quiet.wav", 0.125);
    let config = serde_json::json!({ "samples": [{ "key": 36, "asset": kick }] });
    let json = document("sample_kit", config);
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine
        .load_invention(Invention::from_json(&json).unwrap())
        .unwrap();
    let mut block = [0.0f32; 128];
    engine.set_control("m", "play", 36.0.into()).unwrap();
    engine.render_interleaved(&mut block).unwrap();
    engine
        .set_control("m", "asset.0", quiet.as_str().into())
        .unwrap();
    engine.render_interleaved(&mut block).unwrap();
    assert!(
        (block[0] - 0.5).abs() < 2e-3,
        "the kick rings on: {}",
        block[0]
    );
}
