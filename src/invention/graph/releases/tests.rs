//! An unpatched input glides to rest over ten milliseconds rather than
//! stepping there, whichever way the edit unpatches it.

use serde_json::json;

use crate::RenderEngine;

/// Ten milliseconds at the tests' 48 kHz.
const RELEASE: usize = 480;

/// A still square wave (a constant 1 for the first seconds) through vca
/// `voice`, whose `level` is patched from vca `cv` holding a still square at
/// 0.25: the dac plays 0.25 until the patch goes, then `voice`'s own level,
/// 1. A block of 128 frames.
fn patched() -> RenderEngine {
    let still = |id: &str| {
        json!({ "id": id, "type": "oscillator",
                "config": { "frequency": 0.001, "waveform": "square" } })
    };
    let invention = json!({
        "version": "1.0.0",
        "modules": [
            still("a"), still("b"),
            { "id": "cv", "type": "vca", "config": { "level": 0.25 } },
            { "id": "voice", "type": "vca" },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            { "from": "a", "from_port": "audio", "to": "cv", "to_port": "audio" },
            { "from": "cv", "from_port": "audio", "to": "voice", "to_port": "level" },
            { "from": "b", "from_port": "audio", "to": "voice", "to_port": "audio" },
            { "from": "voice", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    });
    let mut engine = RenderEngine::new(48_000);
    engine.set_block_size(128);
    engine.load_json(&invention.to_string()).unwrap();
    engine
}

/// The left channel of the next `frames` frames.
fn render(engine: &mut RenderEngine, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames * 2];
    engine.render_interleaved(&mut out).unwrap();
    out.chunks(2).map(|frame| frame[0]).collect()
}

/// The largest change from one frame to the next, `before` the first.
fn largest_step(before: f32, out: &[f32]) -> f32 {
    let mut last = before;
    let mut largest = 0.0f32;
    for &x in out {
        largest = largest.max((x - last).abs());
        last = x;
    }
    largest
}

/// `out` glides from 0.25 to 1 in even steps over the release, and stays.
fn assert_glides_to_rest(out: &[f32]) {
    let even = 0.75 / RELEASE as f32;
    assert!(
        largest_step(0.25, out) <= even * 1.01,
        "largest step {} past an even glide's {even}",
        largest_step(0.25, out)
    );
    assert!(
        out[0] > 0.25 && out[RELEASE / 2] < 1.0,
        "no glide: {:?}",
        &out[..4]
    );
    for (i, &x) in out.iter().enumerate().skip(RELEASE - 1) {
        assert!((x - 1.0).abs() < 1e-6, "frame {i}: {x}, not at rest");
    }
}

#[test]
fn a_disconnected_input_glides_to_where_it_rests() {
    let mut engine = patched();
    assert!(render(&mut engine, 1_024).iter().all(|&x| x == 0.25));
    engine.disconnect("cv", "audio", "voice", "level").unwrap();
    assert_glides_to_rest(&render(&mut engine, 2_048));
}

#[test]
fn removing_the_module_feeding_an_input_releases_it_too() {
    let mut engine = patched();
    render(&mut engine, 1_024);
    engine.remove_module("cv").unwrap();
    assert_glides_to_rest(&render(&mut engine, 2_048));
}

#[test]
fn an_input_patched_again_as_it_releases_is_the_connections_again() {
    let mut engine = patched();
    render(&mut engine, 1_024);
    engine.disconnect("cv", "audio", "voice", "level").unwrap();
    let released = render(&mut engine, 128);
    assert!(released[127] > 0.25);
    engine.connect("cv", "audio", "voice", "level").unwrap();
    let out = render(&mut engine, 2_048);
    assert!(out.iter().all(|&x| x == 0.25), "{:?}", &out[..4]);
}

#[test]
fn an_unpatched_audio_input_fades_rather_than_cuts() {
    let mut engine = patched();
    render(&mut engine, 1_024);
    engine.disconnect("voice", "audio", "dac", "audio").unwrap();
    let out = render(&mut engine, 2_048);
    assert!(largest_step(0.25, &out) <= 0.25 / RELEASE as f32 * 1.01);
    assert!(out[RELEASE - 1..].iter().all(|&x| x == 0.0));
}
