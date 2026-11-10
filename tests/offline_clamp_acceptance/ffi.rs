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
