//! A voice's release and steal fades on its own, away from the instrument.

use super::*;

/// A voice sounding `levels` (mono) at `ratio`, its loop `start..end` with
/// `crossfade`, already at `position` and held.
fn voice(levels: Vec<f32>, (start, end, crossfade): (f64, f64, f64), ratio: f64) -> Voice {
    let mut voice = Voice::new();
    voice.sample = Some(Arc::new(SampleData::from_interleaved(
        1, 48_000, 48_000, levels,
    )));
    voice.loop_region = Some(ScaledLoop {
        start,
        end,
        crossfade,
    });
    voice.ratio = ratio;
    voice.active = true;
    voice.held = true;
    voice
}

#[test]
fn a_long_fade_at_a_high_rate_finishes_in_the_tail_it_was_judged_to_fit() {
    // 25 s at 192 kHz: 4.8M steps of a slope f32 cannot hold near 1.0. The
    // tail, read at 1/100 speed, lasts 100 frames longer than the fade, so
    // the voice leaves the loop at once and must reach silence before the
    // sample ends rather than be cut there.
    let step = 1.0 / (25.0 * 192_000.0);
    let fade = (1.0 / step) as usize;
    let tail = fade / 100 + 1;
    let mut voice = voice(vec![0.5; 100 + tail], (0.0, 100.0, 0.0), 0.01);
    voice.position = 100.0 - 0.01;
    voice.release(step as f32, false);
    assert!(!voice.wraps, "a tail that holds the fade is played");

    let mut last = voice.release_gain();
    while voice.active {
        last = voice.release_gain();
        voice.advance();
    }
    assert!(last < 1e-3, "the sample ended under a fade at gain {last}");
}

#[test]
fn a_stolen_voice_keeps_wrapping_its_loop_through_the_declick() {
    // The loop (100..200, crossfading over its last 50 frames) holds 0.5;
    // the 10-frame tail is -0.5. A voice stolen just before its loop end
    // must keep reading the loop through the declick, not run into the
    // tail and off the end of the sample.
    let mut levels = vec![0.5; 200];
    levels.extend([-0.5; 10]);
    let mut voice = voice(levels, (100.0, 200.0, 50.0), 1.0);
    voice.position = 190.0;
    voice.release(1.0 / 48_000.0, false);
    assert!(voice.wraps);

    let declick = 1.0 / 144.0;
    voice.begin_steal_fade(1.0, declick);
    voice.active = false;
    let mut last = voice.fade_out().0;
    for frame in 0..200 {
        let level = voice.fade_out().0;
        assert!(
            (level - last).abs() < 0.01,
            "step of {} at declick frame {frame}",
            (level - last).abs()
        );
        last = level;
    }
    assert_eq!(last, 0.0);
}

#[test]
fn a_stolen_note_eases_out_with_no_corner_where_its_fade_starts_or_ends() {
    // A note held at 0.5: a straight fade over 144 frames would bend its
    // slope by 0.5 / 144 (0.0035, a tick) on its first and last frames.
    let mut voice = voice(vec![0.5; 1_000], (0.0, 1_000.0, 0.0), 1.0);
    voice.begin_steal_fade(1.0, 1.0 / 144.0);
    voice.active = false;
    let mut heard = vec![0.5, 0.5];
    while voice.fade_gain > 0.0 {
        heard.push(voice.fade_out().0);
    }
    heard.extend([0.0, 0.0]);
    let bend = heard
        .windows(3)
        .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
        .fold(0.0, f32::max);
    assert!(bend < 0.0005, "bent by {bend}");
}
