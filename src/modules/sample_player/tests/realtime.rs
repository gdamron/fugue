//! The audio thread never waits on a control thread, never frees a sample,
//! and never erases a `play` a control thread wrote.

use std::sync::{mpsc, Arc};
use std::time::Duration;

use super::write_test_wav;
use crate::alloc_counter::allocator_events;
use crate::modules::sample_loading::SampleData;
use crate::modules::sample_player::controls::transport_play;
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

/// The swap lock, held by another thread as a control thread mid-
/// `set_asset` would, until [`Self::release`]: or for two seconds, should
/// the audio thread wait on it, so that fails rather than hangs.
struct HeldSwapLock {
    release: mpsc::Sender<()>,
    holder: std::thread::JoinHandle<bool>,
}

fn hold_swap_lock(controls: &SamplePlayerControls) -> HeldSwapLock {
    let controls = controls.clone();
    let (release, released) = mpsc::channel();
    let (holding, holds) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _held = controls.hold_swap_lock();
        holding.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(2)).is_ok()
    });
    holds.recv_timeout(Duration::from_secs(10)).unwrap();
    HeldSwapLock { release, holder }
}

impl HeldSwapLock {
    fn release(self) {
        let _ = self.release.send(());
        let in_time = self.holder.join().unwrap();
        assert!(in_time, "the audio thread waited on the swap lock");
    }
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

    let held = hold_swap_lock(&controls);
    player.process(1);
    assert!((player.get_output("audio_left").unwrap() - 0.2).abs() < 1e-3);
    held.release();

    player.process(1);
    assert!((player.get_output("audio_left").unwrap() - 0.7).abs() < 1e-3);

    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
}

#[test]
fn a_one_shot_ending_while_its_replacement_waits_leaves_the_replacement_playing() {
    let (controls, mut player, first) = one_shot(&[[0.2, 0.0]; 8]);
    controls.set_play(true);
    player.process(1);
    let second = write_test_wav(44_100, 1, &[[0.7, 0.0]; 8]);
    controls.set_asset(second.to_str().unwrap()).unwrap();

    // The old one-shot plays out while the swap is deferred.
    let held = hold_swap_lock(&controls);
    player.process(16);
    player.process(1);
    assert_eq!(player.get_output("audio_left").unwrap(), 0.0, "ended");
    held.release();

    player.process(1);
    assert!((player.get_output("audio_left").unwrap() - 0.7).abs() < 1e-3);

    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
}

#[test]
fn a_stop_written_while_a_replacement_waits_stays_stopped() {
    let (controls, mut player, first) = one_shot(&[[0.2, 0.0]; 8]);
    controls.set_play(true);
    player.process(1);
    let second = write_test_wav(44_100, 1, &[[0.7, 0.0]; 8]);
    controls.set_asset(second.to_str().unwrap()).unwrap();

    let held = hold_swap_lock(&controls);
    player.process(16);
    controls.set_play(false);
    player.process(1);
    held.release();

    player.process(4);
    assert_eq!(player.get_output("audio_left").unwrap(), 0.0);
    assert!(!controls.play());

    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
}

#[test]
fn a_swap_with_nothing_staged_is_done_while_the_replaced_sample_waits() {
    let (controls, _player, first) = one_shot(&[[0.2, 0.0]; 8]);
    let mut current = None;
    assert_eq!(controls.try_swap(&mut current, || {}), Some(true));

    // A swap can take a sample staged before its version moves; the block
    // after sees the version moved, with nothing staged and the replaced
    // sample still parked. Retrying would keep the player replacing.
    let second = write_test_wav(44_100, 1, &[[0.7, 0.0]; 8]);
    controls.set_asset(second.to_str().unwrap()).unwrap();
    let mut refreshed = false;
    assert_eq!(controls.try_swap(&mut current, || {}), Some(true));
    let swapped = controls.try_swap(&mut current, || refreshed = true);
    assert_eq!((swapped, refreshed), (Some(false), false));

    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
}

#[test]
fn a_swap_reads_play_and_loop_as_they_stood_once_its_sample_was_staged() {
    let (controls, _player, first) = one_shot(&[[0.2, 0.0]; 8]);
    let mut current = None;
    assert_eq!(controls.try_swap(&mut current, || {}), Some(true));
    controls.set_play(true);
    // A block reads `play` on and `loop` off; then a stop, looping, and a
    // new sample.
    let (mut transport, mut looping) = (controls.transport(), controls.loop_enabled());
    controls.set_play(false);
    controls.set_loop_enabled(true);
    let second = write_test_wav(44_100, 1, &[[0.7, 0.0]; 8]);
    controls.set_asset(second.to_str().unwrap()).unwrap();
    let mut locked = false;
    let swapped = controls.try_swap(&mut current, || {
        locked = controls.swap_lock_held();
        transport = controls.transport();
        looping = controls.loop_enabled();
    });
    assert_eq!(swapped, Some(true));
    assert!(locked, "read again under the lock");
    assert!(!transport_play(transport), "the new sample starts stopped");
    assert!(looping, "and loops");

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
