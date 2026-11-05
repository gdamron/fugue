use super::*;

const SR: u32 = 48_000;

fn tone(n: usize, hz: f32) -> f32 {
    (std::f32::consts::TAU * hz * n as f32 / SR as f32).sin()
}

fn filled(frames: usize, hz: f32) -> Recording {
    let mut buffer = Recording::new(SR, frames);
    for n in 0..frames * 2 {
        let x = tone(n, hz);
        buffer.write(x, x, true);
    }
    buffer
}

fn largest_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

fn gain_sum(stretcher: &Stretcher) -> f32 {
    stretcher
        .voices
        .iter()
        .filter(|voice| voice.active)
        .map(Voice::gain)
        .sum()
}

/// How far behind the write head each playing window reads.
fn behind<'a>(stretcher: &'a Stretcher, buffer: &Recording) -> impl Iterator<Item = f64> + 'a {
    let (head, length) = (buffer.head() as f64, buffer.len() as f64);
    stretcher
        .voices
        .iter()
        .filter(|voice| voice.active)
        .map(move |voice| (head - voice.at).rem_euclid(length))
}

#[test]
fn overlapping_windows_hold_a_frozen_tone_steady() {
    let buffer = filled(SR as usize, 220.0);
    let mut stretcher = Stretcher::new(SR);
    let shape = StretchShape {
        position: 0.5,
        size: 0.5,
        ratio: 1.0,
    };
    let out: Vec<f32> = (0..SR as usize)
        .map(|_| stretcher.tick(&buffer, &shape, false).0)
        .collect();
    let settled = &out[SR as usize / 4..];
    let peak = settled.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((0.9..1.1).contains(&peak), "a peak of {peak}");
    // The windows line up: no step beyond the tone's own.
    let tone = std::f32::consts::TAU * 220.0 / SR as f32;
    assert!(
        largest_step(settled) <= 1.5 * tone,
        "{}",
        largest_step(settled)
    );
}

#[test]
fn the_windows_sum_to_one_at_every_size_and_through_cuts() {
    // Window lengths whose phase never quite reached one used to skip
    // every third window; a size moving between windows bent the sum.
    let buffer = filled(SR as usize, 220.0);
    let mut stretcher = Stretcher::new(SR);
    for n in 0..SR as usize * 2 {
        let shape = StretchShape {
            position: 0.4,
            size: (n as f32 / 7_000.0).fract(),
            ratio: 1.0,
        };
        if n % 9_000 == 4_500 {
            stretcher.restart();
        }
        stretcher.tick(&buffer, &shape, false);
        if n > 6_000 {
            let sum = gain_sum(&stretcher);
            assert!((sum - 1.0).abs() < 1e-4, "a sum of {sum} at {n}");
        }
    }
}

#[test]
fn a_pitch_jump_while_recording_keeps_clear_of_the_head() {
    let mut buffer = Recording::new(SR, SR as usize);
    let mut stretcher = Stretcher::new(SR);
    let length = buffer.len() as f64;
    // Up two octaves at the newest audio, then down at the oldest.
    let moves = [(0.0, 1.0), (0.0, 4.0), (1.0, 4.0), (1.0, 0.25)];
    let mut n = 0;
    for (position, ratio) in moves {
        let shape = StretchShape {
            position,
            size: 1.0,
            ratio,
        };
        for _ in 0..SR as usize {
            let x = tone(n, 220.0);
            n += 1;
            stretcher.tick(&buffer, &shape, true);
            buffer.write(x, x, true);
            for delay in behind(&stretcher, &buffer) {
                assert!(
                    (3.0..=length - 3.0).contains(&delay),
                    "a window {delay} behind the head at ratio {ratio}"
                );
            }
        }
    }
}

#[test]
fn unfreezing_cuts_away_from_windows_the_head_would_overrun() {
    let mut buffer = filled(SR as usize / 2, 220.0);
    for _ in 0..SR as usize / 10 {
        buffer.write(0.0, 0.0, false);
    }
    let length = buffer.len() as f64;
    let mut stretcher = Stretcher::new(SR);
    let shape = StretchShape {
        position: 0.0,
        size: 0.0,
        ratio: 0.5,
    };
    for _ in 0..SR as usize / 10 {
        stretcher.tick(&buffer, &shape, false);
    }
    // A frozen window reading just past the still head, below unity: the
    // head would catch it within a few hundred frames of moving again.
    let head = buffer.head() as f64;
    for voice in stretcher.voices.iter_mut().filter(|voice| voice.active) {
        voice.at = (head + 100.0) % length;
        voice.ratio = 0.5;
    }
    stretcher.thaw(&buffer);
    for n in 0..SR as usize / 4 {
        stretcher.tick(&buffer, &shape, true);
        buffer.write(tone(n, 220.0), 0.0, true);
        for delay in behind(&stretcher, &buffer) {
            assert!((3.0..=length - 3.0).contains(&delay), "{delay} at {n}");
        }
    }
}

#[test]
fn a_ratio_of_two_plays_an_octave_up() {
    let buffer = filled(SR as usize, 200.0);
    let mut stretcher = Stretcher::new(SR);
    let shape = StretchShape {
        position: 0.3,
        size: 1.0,
        ratio: 2.0,
    };
    let out: Vec<f32> = (0..SR as usize)
        .map(|_| stretcher.tick(&buffer, &shape, false).0)
        .collect();
    let rises = out[SR as usize / 2..]
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count();
    // Half a second of 400 Hz.
    assert!((195..=205).contains(&rises), "{rises} cycles");
}

#[test]
fn cuts_in_a_tiny_buffer_never_run_out_of_windows() {
    // A short buffer caps the hop below the cut's own length.
    let mut buffer = Recording::new(SR, 1_500);
    let mut stretcher = Stretcher::new(SR);
    let shape = StretchShape {
        position: 0.5,
        size: 1.0,
        ratio: 1.0,
    };
    for n in 0..20_000 {
        if n % 97 == 0 {
            stretcher.restart();
        }
        buffer.write(tone(n, 220.0), 0.0, true);
        stretcher.tick(&buffer, &shape, true);
        let sum = gain_sum(&stretcher);
        assert!((sum - 1.0).abs() < 1e-4, "a sum of {sum} at {n}");
    }
}
