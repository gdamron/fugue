//! The audio thread never waits on a control thread, never frees a sample,
//! and never erases a `play` a control thread wrote.

use std::sync::Arc;

use super::write_test_wav;
use crate::alloc_counter::allocator_events;
use crate::modules::sample_loading::SampleData;
use crate::modules::sample_player::{SamplePlayer, SamplePlayerControls};
use crate::Module;

fn one_shot(frames: &[[f32; 2]]) -> (SamplePlayerControls, SamplePlayer, std::path::PathBuf) {
    let path = write_test_wav(44_100, 1, frames);
    let controls = SamplePlayerControls::new(
        44_100,
        Some(path.to_str().unwrap()),
        Some(false),
        Some(false),
    )
    .unwrap();
    let player = SamplePlayer::new_with_controls(controls.clone());
    (controls, player, path)
}

#[test]
fn a_retrigger_written_as_a_one_shot_ends_is_not_lost() {
    let (controls, mut player, path) = one_shot(&[[0.4, 0.0], [0.4, 0.0]]);
    controls.set_play(true);
    player.process(1);

    // The block that reaches the end read `play` at its top; a retrigger
    // lands while it runs. The audio thread's end-of-sample write loses.
    let observed = controls.transport();
    controls.set_play(true);
    assert!(!controls.end_play(observed));
    assert!(controls.play());

    // The retrigger restarts playback on the next block.
    player.process(1);
    assert_eq!(player.get_output("start").unwrap(), 1.0);
    assert!(player.get_output("audio_left").unwrap() > 0.3);

    let _ = std::fs::remove_file(path);
}

#[test]
fn a_stop_written_as_a_one_shot_ends_stays_stopped() {
    let (controls, mut player, path) = one_shot(&[[0.4, 0.0], [0.4, 0.0]]);
    controls.set_play(true);
    player.process(1);

    let observed = controls.transport();
    controls.set_play(false);
    assert!(!controls.end_play(observed));
    assert!(!controls.play());

    let _ = std::fs::remove_file(path);
}

#[test]
fn a_one_shot_reaching_its_end_turns_play_off() {
    let (controls, mut player, path) = one_shot(&[[0.4, 0.0]]);
    controls.set_play(true);
    player.process(2);
    assert!(!controls.play());

    // `play = true` again starts it over.
    controls.set_play(true);
    player.process(1);
    assert_eq!(player.get_output("start").unwrap(), 1.0);

    let _ = std::fs::remove_file(path);
}

#[test]
fn a_busy_swap_lock_defers_the_swap_instead_of_blocking() {
    let (controls, mut player, first) = one_shot(&[[0.2, 0.0]; 8]);
    controls.set_loop_enabled(true);
    controls.set_play(true);
    player.process(1);
    assert!((player.get_output("audio_left").unwrap() - 0.2).abs() < 1e-3);

    let second = write_test_wav(44_100, 1, &[[0.7, 0.0]; 8]);
    controls.set_asset(second.to_str().unwrap()).unwrap();

    let held = controls.hold_swap_lock();
    player.process(1);
    assert!((player.get_output("audio_left").unwrap() - 0.2).abs() < 1e-3);
    drop(held);

    player.process(1);
    assert!((player.get_output("audio_left").unwrap() - 0.7).abs() < 1e-3);

    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
}

#[test]
fn replacing_a_sample_frees_nothing_on_the_audio_thread() {
    let (controls, mut player, first) = one_shot(&[[0.2, 0.0]; 8]);
    controls.set_play(true);
    player.process(1);

    // Hold a sample no cache shares, so dropping it would free its buffers.
    let unique = Arc::new(SampleData::from_interleaved(
        1,
        44_100,
        44_100,
        vec![0.5; 8],
    ));
    let watch = Arc::downgrade(&unique);
    player.sample = Some(unique);

    let second = write_test_wav(44_100, 1, &[[0.7, 0.0]; 8]);
    controls.set_asset(second.to_str().unwrap()).unwrap();
    let (_, allocs, frees) = allocator_events(|| player.process(1));
    assert_eq!((allocs, frees), (0, 0));
    assert!((player.get_output("audio_left").unwrap() - 0.7).abs() < 1e-3);
    assert!(watch.upgrade().is_some(), "parked for a control thread");

    // The next swap, on a control thread, drops it.
    controls.set_asset(first.to_str().unwrap()).unwrap();
    assert!(watch.upgrade().is_none());

    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
}
