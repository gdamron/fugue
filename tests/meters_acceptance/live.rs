//! Live meter: RMS post-clamp, peak pre-clamp.

use super::support::*;

/// Plays `json` through a capturing backend and returns the meter window
/// with the raw graph output captured over the same frames.
fn play(json: &str, frames: usize) -> (fugue::MasterLevels, Vec<f32>) {
    let live = Live::start(json);
    let raw = live.capture(frames);
    let meter = live.running.master_meter();
    live.running.stop();
    assert_eq!(
        meter.frames as usize, frames,
        "the window holds the capture"
    );
    (meter, raw)
}

fn device_samples(raw: &[f32]) -> Vec<f32> {
    raw.iter().map(|x| device(*x)).collect()
}

const FRAMES: usize = 9_600;

#[test]
fn over_unity_rms_is_measured_after_the_clamp() {
    // 12 dB: the tone peaks near 4.0 and the clamp flattens most of each cycle.
    for (waveform, db) in [("sine", 12.0), ("sawtooth", 6.0), ("sine", 3.0)] {
        let json = tone(waveform, 311.0, db);
        let (meter, raw) = play(&json, FRAMES);
        assert!(channel_peak(&raw, 0) > 1.3, "{waveform} {db} dB is over");
        let dev = device_samples(&raw);
        for (ch, got) in [(0, meter.left_rms), (1, meter.right_rms)] {
            let expected = player_rms(&dev, ch);
            assert!(expected <= 1.0);
            assert_rms(got, expected, &format!("{waveform} {db} dB channel {ch}"));
            assert!(got.unwrap() <= 1.0);
        }
    }
}

#[test]
fn over_unity_rms_matches_the_players_rms_of_the_offline_render() {
    let json = tone("sine", 311.0, 12.0);
    let (meter, _) = play(&json, FRAMES);
    let rendered = offline(&json, FRAMES, 256);
    for (ch, got) in [(0, meter.left_rms), (1, meter.right_rms)] {
        assert_rms(got, player_rms(&rendered, ch), &format!("channel {ch}"));
    }
}

#[test]
fn over_unity_peak_stays_pre_clamp() {
    let json = tone("sine", 311.0, 12.0);
    let (meter, raw) = play(&json, FRAMES);
    assert!(meter.left_peak > 3.0, "{}", meter.left_peak);
    assert!(meter.right_peak > 3.0, "{}", meter.right_peak);
    assert!(f64::from(meter.left_peak - channel_peak(&raw, 0)).abs() < 1e-3);
    assert!(f64::from(meter.left_rms.unwrap()) <= 1.0);
    assert!(f64::from(meter.right_rms.unwrap()) <= 1.0);
}

#[test]
fn quiet_piece_pre_and_post_clamp_rms_agree() {
    let json = tone("sine", 311.0, -9.0);
    let (meter, raw) = play(&json, FRAMES);
    assert!(channel_peak(&raw, 0) < 0.5 && channel_peak(&raw, 0) > 0.1);
    for (ch, got) in [(0, meter.left_rms), (1, meter.right_rms)] {
        // Nothing is clamped, so the raw and device RMS are the same number.
        let expected = player_rms(&raw, ch);
        assert_rms(got, expected, &format!("quiet channel {ch}"));
        assert_rms(got, player_rms(&device_samples(&raw), ch), "quiet device");
    }
    assert!(meter.left_peak < 0.5);
}

#[test]
fn non_finite_samples_count_as_silence_in_the_rms() {
    let (meter, raw) = play(&overflowing_chain(), FRAMES);
    let dev = device_samples(&raw);
    assert!(dev.iter().all(|x| x.is_finite()));
    for (ch, got) in [(0, meter.left_rms), (1, meter.right_rms)] {
        let expected = player_rms(&dev, ch);
        assert!(expected > 0.0 && expected <= 1.0);
        assert_rms(got, expected, &format!("overflow channel {ch}"));
    }
}
