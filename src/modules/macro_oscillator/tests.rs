use super::*;
use crate::ModuleRegistry;
use serde_json::json;

const SR: u32 = 48_000;

fn build(config: serde_json::Value) -> Box<dyn Module + Send> {
    build_at(SR, config)
}

fn build_at(sample_rate: u32, config: serde_json::Value) -> Box<dyn Module + Send> {
    match ModuleRegistry::default()
        .build(TYPE_ID, sample_rate, &config)
        .expect("builds")
        .module
    {
        GraphModule::Module(module) => module,
        _ => panic!("a plain module"),
    }
}

fn render(module: &mut dyn Module, frames: usize, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    while out.len() < frames {
        let n = block.min(frames - out.len());
        module.process(n);
        out.extend_from_slice(&module.output_block(0)[..n]);
    }
    out
}

/// Rising zero crossings per second, after the first tenth of a second.
fn crossing_rate(x: &[f32]) -> f32 {
    let x = &x[SR as usize / 10..];
    let crossings = x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    crossings as f32 * SR as f32 / x.len() as f32
}

#[test]
fn virtual_analog_plays_at_its_frequency() {
    for hz in [110.0, 440.0, 1760.0] {
        let mut module = build(json!({ "frequency": hz, "timbre": 0.0, "morph": 0.0 }));
        let x = render(module.as_mut(), SR as usize, 64);
        let rate = crossing_rate(&x);
        assert!((rate / hz - 1.0).abs() < 0.01, "{hz} Hz plays at {rate}");
    }
}

/// Plays a trigger every 0.1 s with a vibrato on `frequency_mod`, in
/// host blocks of `block` frames.
fn play_patched(block: usize, frames: usize) -> Vec<f32> {
    let mut module = build(json!({ "frequency": 330.0, "frequency_mod_depth": 20.0 }));
    module.set_input_connected(FREQUENCY_MOD_IN, true);
    module.set_input_connected(TRIGGER_IN, true);
    let mut out = Vec::with_capacity(frames);
    while out.len() < frames {
        let n = block.min(frames - out.len());
        for i in 0..n {
            let t = out.len() + i;
            module.input_block_mut(FREQUENCY_MOD_IN)[i] = (t as f32 * 0.001).sin();
            module.input_block_mut(TRIGGER_IN)[i] = if t % 4_800 < 100 { 1.0 } else { 0.0 };
        }
        module.process(n);
        out.extend_from_slice(&module.output_block(0)[..n]);
    }
    out
}

#[test]
fn block_size_does_not_change_the_sound() {
    let reference = play_patched(64, 24_000);
    for block in [1, 7, 12, 100, 1024] {
        assert!(
            play_patched(block, 24_000) == reference,
            "blocks of {block}"
        );
    }
}

#[test]
fn inputs_that_are_not_numbers_pass_without_sticking() {
    let mut module = build(json!({}));
    for port in [FREQUENCY_IN, FREQUENCY_MOD_IN, LEVEL_IN] {
        module.set_input_connected(port, true);
    }
    for bad in [f32::NAN, f32::INFINITY, f32::MAX] {
        for port in [FREQUENCY_IN, FREQUENCY_MOD_IN, LEVEL_IN] {
            module.input_block_mut(port)[..64].fill(bad);
        }
        module.process(64);
    }
    module.input_block_mut(FREQUENCY_IN)[..64].fill(220.0);
    module.input_block_mut(FREQUENCY_MOD_IN)[..64].fill(0.0);
    module.input_block_mut(LEVEL_IN)[..64].fill(1.0);
    let x = render(module.as_mut(), 9_600, 64);
    assert!(x.iter().all(|s| s.is_finite()));
    let peak = x[4_800..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.1, "recovers to {peak}");
}

#[test]
fn a_connected_frequency_overrides_the_control() {
    let mut module = build(json!({ "frequency": 440.0, "timbre": 0.0, "morph": 0.0 }));
    module.set_input_connected(FREQUENCY_IN, true);
    let mut out = Vec::new();
    for _ in 0..SR as usize / 64 {
        module.input_block_mut(FREQUENCY_IN)[..64].fill(220.0);
        module.process(64);
        out.extend_from_slice(&module.output_block(0)[..64]);
    }
    let rate = crossing_rate(&out);
    assert!((rate / 220.0 - 1.0).abs() < 0.01, "plays at {rate}");
}

#[test]
fn decay_reads_back_in_seconds_and_clamps() {
    let mut module = build(json!({ "decay": 0.5 }));
    assert_eq!(module.get_control("decay"), Ok(0.5));
    module.set_control("decay", 100.0).unwrap();
    assert_eq!(module.get_control("decay"), Ok(MAX_DECAY));
}

#[test]
fn model_takes_a_name_in_config_and_refuses_an_unknown_position() {
    let mut module = build(json!({ "model": "virtual_analog" }));
    assert_eq!(module.get_control("model"), Ok(0.0));
    assert!(module.set_control("model", MODEL_COUNT as f32).is_err());
}

#[test]
fn decay_keeps_its_time_at_other_sample_rates() {
    for rate in [44_100, 96_000] {
        let mut module = build_at(rate, json!({ "decay": 0.5 }));
        module.set_input_connected(TRIGGER_IN, true);
        module.input_block_mut(TRIGGER_IN)[..64].fill(1.0);
        module.process(64);
        module.input_block_mut(TRIGGER_IN)[..64].fill(0.0);
        let at = |seconds: f32| (seconds * rate as f32) as usize;
        let x = render(module.as_mut(), at(0.6), 64);
        let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let early = peak(&x[..at(0.05)]);
        assert!(
            peak(&x[at(0.4)..at(0.45)]) > early * 2e-3,
            "{rate} Hz closes early"
        );
        assert!(peak(&x[at(0.55)..]) < early * 2e-3, "{rate} Hz rings on");
    }
}

#[test]
fn a_trigger_pings_the_gate() {
    let mut module = build(json!({ "decay": 0.5 }));
    module.set_input_connected(TRIGGER_IN, true);
    module.input_block_mut(TRIGGER_IN)[..64].fill(1.0);
    module.process(64);
    module.input_block_mut(TRIGGER_IN)[..64].fill(0.0);
    let early = render(module.as_mut(), 2_400, 64);
    // 60 dB down by its decay time (0.05 s already passed).
    let held = render(module.as_mut(), 19_200, 64);
    let late = render(module.as_mut(), 4_800, 64);
    let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak(&early) > 0.1, "sounds at {}", peak(&early));
    assert!(
        peak(&held[..2_400]) > 0.01,
        "still sounding at {}",
        peak(&held)
    );
    assert!(
        peak(&late) < peak(&early) * 2e-3,
        "fades to {}",
        peak(&late)
    );
}

#[test]
fn every_model_stays_bounded_at_every_macro_corner() {
    for model in engines::MODELS {
        for hz in [30.0, 440.0, 6000.0] {
            for corner in 0..27 {
                let [h, t, m] = [corner % 3, corner / 3 % 3, corner / 9].map(|v| v as f32 * 0.5);
                let mut module = build(json!({
                    "model": model, "frequency": hz, "harmonics": h, "timbre": t, "morph": m,
                }));
                let x = render(module.as_mut(), SR as usize / 10, 256);
                let peak = x.iter().fold(0.0f32, |p, v| p.max(v.abs()));
                assert!(
                    peak.is_finite() && peak < 1.5,
                    "{model} {hz} Hz {h}/{t}/{m}: {peak}"
                );
            }
        }
    }
}

/// The levels of the first eight harmonics of `hz` in `x` (Hann
/// windowed), summing to one: the waveform's shape, whatever its phase.
fn harmonic_profile(x: &[f32], hz: f32) -> [f32; 8] {
    let n = x.len() as f32;
    let mut levels = [0.0f32; 8];
    for (k, level) in levels.iter_mut().enumerate() {
        let w = std::f32::consts::TAU * hz * (k + 1) as f32 / SR as f32;
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, &v) in x.iter().enumerate() {
            let hann = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n).cos();
            re += v * hann * (w * i as f32).cos();
            im += v * hann * (w * i as f32).sin();
        }
        *level = re.hypot(im);
    }
    let total: f32 = levels.iter().sum::<f32>().max(1e-9);
    levels.map(|l| l / total)
}

#[test]
fn each_macro_changes_the_new_models_sound() {
    let play = |model: &str, key: &str, value: f32| {
        let mut module = build(json!({ "model": model, "frequency": 220.0, key: value }));
        let x = render(module.as_mut(), SR as usize / 10, 64);
        harmonic_profile(&x[SR as usize / 50..], 220.0)
    };
    for model in [
        "waveshaping",
        "fm",
        "grain_formant",
        "additive",
        "chords",
        "speech",
    ] {
        for key in ["harmonics", "timbre", "morph"] {
            let (low, high) = (play(model, key, 0.1), play(model, key, 0.9));
            let change: f32 = low.iter().zip(&high).map(|(a, b)| (a - b).abs()).sum();
            assert!(change > 0.05, "{model}: {key} does nothing ({change})");
        }
    }
}

/// The level of `hz` in `x`, by a Hann-windowed single-bin DFT.
fn level_at(x: &[f32], hz: f32) -> f32 {
    let n = x.len() as f32;
    let w = std::f32::consts::TAU * hz / SR as f32;
    let (mut re, mut im) = (0.0f32, 0.0f32);
    for (i, &v) in x.iter().enumerate() {
        let hann = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n).cos();
        re += v * hann * (w * i as f32).cos();
        im += v * hann * (w * i as f32).sin();
    }
    re.hypot(im) / n
}

#[test]
fn chords_are_built_in_the_key_and_scale() {
    // A triad on A 220: in C major it is A minor (C 261.6 Hz), in A major
    // A major (C# 277.2 Hz).
    let play = |key: &str, scale: &str| {
        let mut module = build(json!({
            "model": "chords", "frequency": 220.0, "harmonics": 0.45,
            "key": key, "scale": scale,
        }));
        let x = render(module.as_mut(), SR as usize / 2, 256);
        let x = &x[SR as usize / 10..];
        (level_at(x, 261.63), level_at(x, 277.18))
    };
    let (minor_third, major_third) = play("C", "major");
    assert!(
        minor_third > 10.0 * major_third,
        "{minor_third} {major_third}"
    );
    let (minor_third, major_third) = play("A", "major");
    assert!(
        major_third > 10.0 * minor_third,
        "{minor_third} {major_third}"
    );
    let (minor_third, major_third) = play("A", "minor");
    assert!(
        minor_third > 10.0 * major_third,
        "{minor_third} {major_third}"
    );
}
