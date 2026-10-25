//! A channel that stops being heard fades out from where it was, over one
//! glide, rather than stopping at once.

use super::tests::{ramp_frames, BLOCK};
use super::*;

/// A tone slow enough that its own frame-to-frame change is small.
const TONE_HZ: f32 = 220.0;

/// Plays `blocks` blocks of the tone into channel `ch` (and silence into
/// the others it has), from frame `start`, and returns the left output.
fn play(mixer: &mut Mixer, ch: usize, start: usize, blocks: usize, fed: bool) -> Vec<f32> {
    let mut out = Vec::new();
    for b in 0..blocks {
        if fed {
            let block = mixer.input_block_mut(ch);
            for (i, sample) in block[..BLOCK].iter_mut().enumerate() {
                let n = (start + b * BLOCK + i) as f32;
                *sample = (std::f32::consts::TAU * TONE_HZ * n / DEFAULT_SAMPLE_RATE as f32).sin();
            }
        }
        mixer.process(BLOCK);
        out.extend_from_slice(&mixer.output_block(0)[..BLOCK]);
    }
    out
}

/// What the graph does as an edit takes channel `ch`'s input away.
fn disconnect(mixer: &mut Mixer, ch: usize) {
    mixer.input_block_mut(ch).fill(0.0);
    mixer.set_input_connected(ch, false);
}

/// The largest change between two neighbouring frames.
fn steepest(frames: &[f32]) -> f32 {
    frames
        .windows(2)
        .map(|p| (p[1] - p[0]).abs())
        .fold(0.0, f32::max)
}

/// The tone's own steepest frame-to-frame change, at the centre's gain.
fn tone_slope() -> f32 {
    std::f32::consts::TAU * TONE_HZ / DEFAULT_SAMPLE_RATE as f32 * 2.0_f32.sqrt().recip()
}

/// Checks `before` then `after` join without a step and `after` falls
/// silent once a glide has passed.
fn fades_out(before: &[f32], after: &[f32]) {
    let mut across = before[before.len() - 1..].to_vec();
    across.extend_from_slice(after);
    assert!(
        steepest(&across) <= tone_slope() * 1.05,
        "{}",
        steepest(&across)
    );
    let fade = ramp_frames() as usize;
    assert!(after[..fade].iter().any(|&v| v != 0.0));
    assert!(
        after[fade..].iter().all(|&v| v == 0.0),
        "{:?}",
        &after[fade..fade + 4]
    );
}

fn connected(channels: usize) -> Mixer {
    let mut mixer = Mixer::new(channels);
    for ch in 0..channels {
        mixer.set_input_connected(ch, true);
    }
    mixer
}

#[test]
fn a_channel_whose_input_goes_fades_out() {
    let mut mixer = connected(1);
    // Ends mid-cycle, on the tone's way down.
    let before = play(&mut mixer, 0, 0, 3, true);
    disconnect(&mut mixer, 0);
    let after = play(&mut mixer, 0, 0, 8, false);
    fades_out(&before, &after);
}

#[test]
fn a_channel_a_rebuild_drops_fades_out() {
    let mut old = connected(2);
    let before = play(&mut old, 1, 0, 3, true);
    let mut new = Mixer::new(1);
    assert!(new.inherit(&old));
    new.set_input_connected(0, true);
    let after = play(&mut new, 0, 0, 8, false);
    fades_out(&before, &after);
}

#[test]
fn a_tail_carries_on_through_a_rebuild_as_it_fades() {
    let mut old = connected(1);
    let before = play(&mut old, 0, 0, 3, true);
    disconnect(&mut old, 0);
    let mut during = play(&mut old, 0, 0, 1, false);
    let mut new = Mixer::new(1);
    new.inherit(&old);
    during.extend(play(&mut new, 0, 0, 8, false));
    let mut across = before[before.len() - 1..].to_vec();
    across.extend_from_slice(&during);
    assert!(
        steepest(&across) <= tone_slope() * 1.05,
        "{}",
        steepest(&across)
    );
    assert!(during[ramp_frames() as usize..].iter().all(|&v| v == 0.0));
}

#[test]
fn a_channel_connected_again_before_the_next_block_has_no_tail() {
    let mut mixer = connected(1);
    play(&mut mixer, 0, 0, 3, true);
    disconnect(&mut mixer, 0);
    mixer.set_input_connected(0, true);
    let after = play(&mut mixer, 0, 0, 1, false);
    assert!(after.iter().all(|&v| v == 0.0), "{after:?}");
}

#[test]
fn a_channel_kept_through_a_rebuild_has_no_tail() {
    let mut old = connected(1);
    play(&mut old, 0, 0, 3, true);
    let mut new = connected(1);
    new.inherit(&old);
    new.set_input_connected(0, true);
    let after = play(&mut new, 0, 0, 1, false);
    assert!(after.iter().all(|&v| v == 0.0), "{after:?}");
}

#[test]
fn an_input_that_is_not_a_number_leaves_no_tail() {
    let mut mixer = connected(1);
    mixer.input_block_mut(0)[..BLOCK].fill(f32::NAN);
    mixer.process(BLOCK);
    disconnect(&mut mixer, 0);
    let after = play(&mut mixer, 0, 0, 1, false);
    assert!(after.iter().all(|&v| v == 0.0), "{after:?}");
}

#[test]
fn a_quiet_bright_channel_leaves_no_louder_tail() {
    // A 12 kHz tone at a thousandth of full scale, ending between a peak
    // and a zero crossing: its last slope is as steep as it gets.
    const AMPLITUDE: f32 = 0.001;
    let mut mixer = connected(1);
    let block = mixer.input_block_mut(0);
    for (i, sample) in block[..BLOCK].iter_mut().enumerate() {
        *sample = AMPLITUDE * (i as f32 * std::f32::consts::FRAC_PI_2).sin();
    }
    mixer.process(BLOCK);
    let before = mixer.output_block(0)[..BLOCK]
        .iter()
        .fold(0.0, |m: f32, v| m.max(v.abs()));
    disconnect(&mut mixer, 0);
    let after = play(&mut mixer, 0, 0, 8, false);
    let tail = after.iter().fold(0.0, |m: f32, v| m.max(v.abs()));
    assert!(tail <= before, "tail {tail} over the tone's {before}");
}

#[test]
fn a_fading_tail_never_allocates() {
    let mut mixer = connected(4);
    play(&mut mixer, 2, 0, 2, true);
    let mut new = Mixer::new(2);
    let ((), allocs, frees) = crate::alloc_counter::allocator_events(|| {
        disconnect(&mut mixer, 2);
        mixer.process(BLOCK);
        let _ = new.inherit(&mixer);
        new.process(BLOCK);
    });
    assert_eq!((allocs, frees), (0, 0));
}
