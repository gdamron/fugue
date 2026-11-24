//! Offline `take_levels`: RMS post-clamp, peak pre-clamp.

use super::support::*;
use fugue::{OfflineLevels, RenderEngine};

/// Renders `frames` frames in buffers of 256; returns the device samples
/// and the levels read afterwards.
fn render(json: &str, frames: usize) -> (Vec<f32>, OfflineLevels, RenderEngine) {
    let mut engine = RenderEngine::new(RATE);
    engine.set_seed(Some(1));
    engine.load_json(json).unwrap();
    let mut all = Vec::new();
    let mut done = 0;
    while done < frames {
        let n = 256.min(frames - done);
        let mut out = vec![f32::NAN; n * 2];
        engine.render_interleaved(&mut out).unwrap();
        all.extend(out);
        done += n;
    }
    let levels = engine.take_levels().expect("a piece is loaded");
    (all, levels, engine)
}

const FRAMES: usize = 9_600;

#[test]
fn over_unity_rms_is_measured_after_the_clamp() {
    for (waveform, db) in [("sine", 12.0), ("sawtooth", 6.0), ("sine", 3.0)] {
        let (out, levels, _) = render(&tone(waveform, 311.0, db), FRAMES);
        assert_eq!(levels.frames as usize, FRAMES);
        for (ch, got) in [(0, levels.left_rms), (1, levels.right_rms)] {
            let expected = player_rms(&out, ch);
            assert!(expected <= 1.0);
            assert_rms(got, expected, &format!("{waveform} {db} dB channel {ch}"));
            assert!(got.unwrap() <= 1.0);
        }
    }
}

#[test]
fn over_unity_peak_stays_pre_clamp() {
    let (out, levels, _) = render(&tone("sine", 311.0, 12.0), FRAMES);
    assert!(
        channel_peak(&out, 0) < 1.0 + f32::EPSILON,
        "written is clamped"
    );
    assert!(levels.left_peak > 3.0, "{}", levels.left_peak);
    assert!(levels.right_peak > 3.0, "{}", levels.right_peak);
    assert!(levels.left_rms.unwrap() <= 1.0 && levels.right_rms.unwrap() <= 1.0);
}

#[test]
fn quiet_piece_pre_and_post_clamp_rms_agree() {
    let (out, levels, _) = render(&tone("sine", 311.0, -9.0), FRAMES);
    assert!(levels.left_peak < 0.5 && levels.left_peak > 0.1);
    for (ch, got) in [(0, levels.left_rms), (1, levels.right_rms)] {
        assert_rms(got, player_rms(&out, ch), &format!("quiet channel {ch}"));
    }
}

#[test]
fn non_finite_samples_count_as_silence_in_the_rms() {
    let (out, levels, _) = render(&overflowing_chain(), FRAMES);
    assert!(out.iter().all(|x| x.is_finite()));
    for (ch, got) in [(0, levels.left_rms), (1, levels.right_rms)] {
        let expected = player_rms(&out, ch);
        assert!(expected > 0.0 && expected <= 1.0);
        assert_rms(got, expected, &format!("overflow channel {ch}"));
    }
}

#[test]
fn the_next_read_measures_only_the_next_stretch() {
    // After a read the window starts over: a second stretch is measured
    // alone, on the same post-clamp footing.
    let json = tone("sine", 311.0, 12.0);
    let (_, _, mut engine) = render(&json, FRAMES);
    let mut out = vec![0.0f32; 2 * 4_800];
    engine.render_interleaved(&mut out).unwrap();
    let levels = engine.take_levels().unwrap();
    assert_eq!(levels.frames, 4_800);
    assert_rms(levels.left_rms, player_rms(&out, 0), "second stretch");
}
