//! Criterion: the same invention renders through
//! `fugue_engine_render_interleaved` with every sample in [-1, 1].

use super::support::*;
use fugue::ffi::*;

/// `frames` frames through the C interface in calls of `chunk` frames.
fn render_c(json: &str, frames: usize, chunk: usize) -> Vec<f32> {
    unsafe {
        let engine = fugue_engine_new(RATE);
        assert!(!engine.is_null());
        assert_eq!(fugue_engine_load_json(engine, json.as_ptr(), json.len()), 1);
        let mut all = Vec::new();
        let mut done = 0;
        while done < frames {
            let n = chunk.min(frames - done);
            let mut out = vec![f32::NAN; n * 2];
            assert_eq!(
                fugue_engine_render_interleaved(engine, out.as_mut_ptr(), n),
                n
            );
            all.extend(out);
            done += n;
        }
        fugue_engine_free(engine);
        all
    }
}

#[test]
fn a_loud_invention_renders_inside_unit_range() {
    for (waveform, db) in [("sine", 12.0), ("square", 12.0), ("sawtooth", 24.0)] {
        let out = render_c(&tone(waveform, 440.0, db), 9_600, 700);
        for (i, x) in out.iter().enumerate() {
            assert!(
                x.is_finite() && (-1.0..=1.0).contains(x),
                "{waveform}: sample {i} is {x}"
            );
        }
        let high = out.iter().cloned().fold(f32::MIN, f32::max);
        let low = out.iter().cloned().fold(f32::MAX, f32::min);
        assert_eq!(
            (high, low),
            (MAX_BELOW_ONE, -1.0),
            "{waveform} reaches the clamp"
        );
    }
}

#[test]
fn the_c_interface_matches_the_engine() {
    for json in [
        tone("sine", 440.0, 12.0),
        overflowing_chain(),
        tone("sine", 440.0, -9.0),
    ] {
        let c = render_c(&json, 4_800, 480);
        let rust = offline(&json, 4_800, 480);
        assert_eq!(hash(&c), hash(&rust));
    }
}

#[test]
fn the_overflowing_chain_stays_finite_through_the_c_interface() {
    let out = render_c(&overflowing_chain(), 4_800, 4_800);
    assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    assert!(out.iter().filter(|x| **x == 0.0).count() >= out.len() * 80 / 100);
}

#[test]
fn an_in_range_invention_is_unchanged_through_the_c_interface() {
    let json = tone("sine", 440.0, -9.0);
    let c = render_c(&json, 24_000, 512);
    if super::identity::PINNED_PLATFORM {
        assert_eq!(hash(&c), super::identity::SINE);
    }
    assert_eq!(hash(&c), hash(&offline(&json, 24_000, 512)));
}
