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
    patched_with(json!([]), json!([]))
}

/// [`patched`], with `modules` and `connections` added.
fn patched_with(modules: serde_json::Value, connections: serde_json::Value) -> RenderEngine {
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
    let mut invention = invention;
    let extend = |key: &str, more: &serde_json::Value, invention: &mut serde_json::Value| {
        let list = invention[key].as_array_mut().unwrap();
        list.extend(more.as_array().unwrap().iter().cloned());
    };
    extend("modules", &modules, &mut invention);
    extend("connections", &connections, &mut invention);
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

#[test]
fn a_control_written_as_its_input_releases_moves_where_it_rests() {
    let mut engine = patched();
    render(&mut engine, 1_024);
    engine.disconnect("cv", "audio", "voice", "level").unwrap();
    let mut out = render(&mut engine, 128);
    engine
        .set_control("voice", "level", crate::ControlValue::Number(0.5))
        .unwrap();
    out.extend(render(&mut engine, 2_048));
    let even = 0.75 / RELEASE as f32;
    assert!(largest_step(0.25, &out) <= even * 1.01);
    for (i, &x) in out.iter().enumerate().skip(128 + RELEASE) {
        assert!(
            (x - 0.5).abs() < 1e-6,
            "frame {i}: {x}, not at the level written"
        );
    }
}

#[test]
fn a_release_follows_its_module_when_a_later_edit_moves_it() {
    let mut engine = patched();
    render(&mut engine, 1_024);
    engine.disconnect("cv", "audio", "voice", "level").unwrap();
    let mut out = render(&mut engine, 128);
    // The first module goes: `voice` moves down the order mid-release.
    engine.remove_module("a").unwrap();
    out.extend(render(&mut engine, 2_048));
    assert_glides_to_rest(&out);
}

#[test]
fn an_input_in_a_feedback_loop_glides_to_rest_too() {
    // `voice` feeds `b` back through a silent vca: the loop runs a sample
    // at a time.
    let mut engine = patched_with(
        json!([{ "id": "fb", "type": "vca", "config": { "level": 0.0 } }]),
        json!([
            { "from": "voice", "from_port": "audio", "to": "fb", "to_port": "audio" },
            { "from": "fb", "from_port": "audio", "to": "b", "to_port": "frequency_mod" }
        ]),
    );
    // The loop's first frame waits on its feedback; from then on, steady.
    let steady = render(&mut engine, 1_024);
    assert!(steady[1..].iter().all(|&x| x == 0.25), "{:?}", &steady[..4]);
    engine.disconnect("cv", "audio", "voice", "level").unwrap();
    assert_glides_to_rest(&render(&mut engine, 2_048));
}

/// [`patched`] with a second cable into `voice`'s `level`, from vca `cv2`
/// holding the still square at 0.5: the dac plays the sum, 0.75.
fn summed() -> RenderEngine {
    patched_with(
        json!([{ "id": "cv2", "type": "vca", "config": { "level": 0.5 } }]),
        json!([
            { "from": "a", "from_port": "audio", "to": "cv2", "to_port": "audio" },
            { "from": "cv2", "from_port": "audio", "to": "voice", "to_port": "level" }
        ]),
    )
}

/// `out` glides evenly from `from` to `to` over the release, and stays.
fn assert_glides(from: f32, to: f32, out: &[f32]) {
    let even = (to - from).abs() / RELEASE as f32;
    let largest = largest_step(from, out);
    assert!(largest <= even * 1.01, "largest step {largest} past {even}");
    let half = (from + to) / 2.0;
    assert!(
        (out[RELEASE / 2 - 1] - half).abs() < 0.01,
        "no glide: {:?}",
        &out[..4]
    );
    for (i, &x) in out.iter().enumerate().skip(RELEASE - 1) {
        assert!((x - to).abs() < 1e-6, "frame {i}: {x}, not at {to}");
    }
}

#[test]
fn a_cable_removed_from_a_summed_input_glides_out() {
    let mut engine = summed();
    assert!(render(&mut engine, 1_024).iter().all(|&x| x == 0.75));
    engine.disconnect("cv2", "audio", "voice", "level").unwrap();
    assert_glides(0.75, 0.25, &render(&mut engine, 2_048));
}

#[test]
fn removing_one_module_of_a_fan_in_glides_its_cable_out() {
    let mut engine = summed();
    render(&mut engine, 1_024);
    engine.remove_module("cv2").unwrap();
    assert_glides(0.75, 0.25, &render(&mut engine, 2_048));
}

#[test]
fn a_cable_gliding_out_carries_on_through_another_edit() {
    let mut engine = summed();
    render(&mut engine, 1_024);
    engine.disconnect("cv2", "audio", "voice", "level").unwrap();
    let mut out = render(&mut engine, 128);
    // An edit elsewhere, `voice` connected before and after.
    engine.disconnect("a", "audio", "cv2", "audio").unwrap();
    out.extend(render(&mut engine, 2_048));
    assert_glides(0.75, 0.25, &out);
}

#[test]
fn the_last_cable_unpatched_as_another_glides_out_releases_from_both() {
    let mut engine = summed();
    render(&mut engine, 1_024);
    engine.disconnect("cv2", "audio", "voice", "level").unwrap();
    let mut out = render(&mut engine, 128);
    engine.disconnect("cv", "audio", "voice", "level").unwrap();
    out.extend(render(&mut engine, 2_048));
    // Down a quarter of the way to 0.25, then on to rest at 1 without a
    // step: neither glide's own step is more than 0.75 over the release.
    assert!(largest_step(0.75, &out) <= 0.75 / RELEASE as f32 * 1.01);
    for (i, &x) in out.iter().enumerate().skip(128 + RELEASE - 1) {
        assert!((x - 1.0).abs() < 1e-6, "frame {i}: {x}, not at rest");
    }
}

#[test]
fn a_cable_patched_back_as_it_glides_out_is_the_routes_again() {
    let mut engine = summed();
    render(&mut engine, 1_024);
    engine.disconnect("cv2", "audio", "voice", "level").unwrap();
    let released = render(&mut engine, 128);
    assert!(released[127] < 0.75);
    engine.connect("cv2", "audio", "voice", "level").unwrap();
    let out = render(&mut engine, 2_048);
    assert!(out.iter().all(|&x| x == 0.75), "{:?}", &out[..4]);
}
