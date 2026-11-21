//! An offline render's levels: peak before the clamp, the samples it cut,
//! and each read covering only what was rendered since the last.

use super::RenderEngine;

/// A sine at `decibels` into a dac without soft clip, on both channels.
fn tone(decibels: f32) -> String {
    format!(
        r#"{{
        "version": "1.0.0",
        "modules": [
            {{ "id": "osc", "type": "oscillator", "config": {{ "waveform": "sine", "frequency": 375.0 }} }},
            {{ "id": "gain", "type": "gain", "config": {{ "decibels": {decibels} }} }},
            {{ "id": "dac", "type": "dac", "config": {{ "soft_clip": false }} }}
        ],
        "connections": [
            {{ "from": "osc", "from_port": "audio", "to": "gain", "to_port": "audio" }},
            {{ "from": "gain", "from_port": "audio", "to": "dac", "to_port": "audio" }}
        ]
    }}"#
    )
}

fn render(engine: &mut RenderEngine, frames: usize) -> Vec<f32> {
    let mut output = vec![0.0f32; 2 * frames];
    engine.render_interleaved(&mut output).unwrap();
    output
}

#[test]
fn nothing_loaded_has_no_levels() {
    assert_eq!(RenderEngine::new(48_000).take_levels(), None);
}

#[test]
fn a_hot_render_reads_its_peak_before_the_clamp_and_counts_what_it_cut() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(&tone(12.0)).unwrap();
    let output = render(&mut engine, 4_800);
    let levels = engine.take_levels().unwrap();

    assert_eq!(levels.frames, 4_800);
    assert!(
        levels.left_peak > 3.9,
        "pre-clamp peak {}",
        levels.left_peak
    );
    assert!(
        levels.right_peak > 3.9,
        "pre-clamp peak {}",
        levels.right_peak
    );
    // The overs are the clamped samples the render wrote at full scale.
    let at_full_scale = |channel: usize| {
        output
            .iter()
            .skip(channel)
            .step_by(2)
            .filter(|x| x.abs() >= crate::modules::dac::MAX_BELOW_ONE)
            .count() as u64
    };
    assert!(levels.left_overs > 0);
    assert_eq!(levels.left_overs, at_full_scale(0));
    assert_eq!(levels.right_overs, at_full_scale(1));
    assert_eq!(levels.non_finite_samples, 0);
}

#[test]
fn a_quiet_render_has_no_overs_and_its_rms_is_the_sines() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(&tone(-6.0)).unwrap();
    render(&mut engine, 48_000);
    let levels = engine.take_levels().unwrap();

    let amplitude = 10f32.powf(-6.0 / 20.0);
    assert!(
        (levels.left_peak - amplitude).abs() < 1e-3,
        "{}",
        levels.left_peak
    );
    let rms = levels.left_rms.unwrap();
    assert!((rms - amplitude / 2f32.sqrt()).abs() < 1e-3, "{rms}");
    assert_eq!((levels.left_overs, levels.right_overs), (0, 0));
}

#[test]
fn each_read_covers_only_what_was_rendered_since_the_last() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(&tone(12.0)).unwrap();
    render(&mut engine, 960);
    let first = engine.take_levels().unwrap();
    assert_eq!(first.frames, 960);
    assert!(first.left_overs > 0);

    let empty = engine.take_levels().unwrap();
    assert_eq!(
        (empty.frames, empty.left_overs, empty.right_overs),
        (0, 0, 0)
    );
    assert_eq!(empty.left_peak, 0.0);

    // A new load starts its counts afresh.
    render(&mut engine, 960);
    engine.load_json(&tone(-6.0)).unwrap();
    render(&mut engine, 960);
    let reloaded = engine.take_levels().unwrap();
    assert_eq!(reloaded.frames, 960);
    assert_eq!((reloaded.left_overs, reloaded.right_overs), (0, 0));
}
