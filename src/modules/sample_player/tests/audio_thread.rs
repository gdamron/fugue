//! Requests apply on the thread running the player, between blocks: a
//! retrigger is never lost to a one-shot's end, and an asset swap neither
//! allocates, frees nor waits there.

use std::thread;

use super::{read, write, write_test_wav};
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::modules::sample_player::table::ASSET;
use crate::modules::sample_player::{build, SamplePlayer};
use crate::ControlSurface;
use crate::Module;

/// A stopped classic one-shot of `frames`.
fn one_shot(frames: &[[f32; 2]]) -> (SamplePlayer, std::path::PathBuf) {
    let path = write_test_wav(44_100, 1, frames);
    let config = serde_json::json!({ "asset": path.to_str().unwrap() });
    (build(44_100, &config).unwrap().0, path)
}

#[test]
fn a_retrigger_written_as_a_one_shot_ends_is_not_lost() {
    // At the block that ends the sample, and with the end mid-block.
    for block in [1, 4] {
        let (mut player, path) = one_shot(&[[0.4, 0.0], [0.4, 0.0]]);
        write(&mut player, "play", true);
        player.process(1);
        player.process(block);
        assert_eq!(read(&player, "playing"), false.into(), "block {block}");

        write(&mut player, "play", true);
        assert_eq!(read(&player, "playing"), true.into(), "block {block}");
        player.process(1);
        assert_eq!(player.get_output("start").unwrap(), 1.0, "block {block}");
        assert!(player.get_output("audio_left").unwrap() > 0.3);
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_retrigger_while_playing_starts_again_and_a_stop_stays_stopped() {
    let (mut player, path) = one_shot(&[[0.4, 0.0]; 8]);
    write(&mut player, "play", true);
    player.process(3);
    write(&mut player, "play", true);
    player.process(1);
    assert_eq!(player.get_output("start").unwrap(), 1.0);

    write(&mut player, "play", false);
    assert_eq!(read(&player, "playing"), false.into());
    player.process(16);
    assert_eq!(player.get_output("audio_left").unwrap(), 0.0);
    assert_eq!(read(&player, "playing"), false.into());

    // A gate started after a stop runs as before.
    player.set_input("play", 1.0).unwrap();
    player.process(1);
    assert_eq!(player.get_output("start").unwrap(), 1.0);
    let _ = std::fs::remove_file(path);
}

#[test]
fn an_asset_swap_during_playback_allocates_frees_and_waits_on_nothing() {
    for mode in ["classic", "elastic"] {
        let first = write_test_wav(44_100, 1, &[[0.2, 0.0]; 64]);
        let second = write_test_wav(44_100, 1, &[[0.6, 0.0]; 64]);
        let config = serde_json::json!({
            "asset": first.to_str().unwrap(),
            "mode": mode,
            "autoplay": true,
            "loop": true,
        });
        let (mut player, surface) = build(44_100, &config).unwrap();
        player.process(8);

        // Loaded on this, the control thread.
        let (payload, _) = surface
            .prepare_payload("asset", &second.to_str().unwrap().into())
            .unwrap()
            .unwrap();
        let replaced = thread::scope(|scope| {
            scope
                .spawn(|| {
                    let (replaced, allocs, frees) = allocator_events(|| {
                        let _scope = AudioThreadScope::enter();
                        let replaced = player.apply_payload(ASSET, payload).ok().unwrap();
                        player.process(4);
                        replaced
                    });
                    assert_eq!((allocs, frees), (0, 0), "{mode}: audio side");
                    replaced
                })
                .join()
                .unwrap()
        });
        // Freed here, off the audio thread.
        drop(replaced);
        assert!(player.get_output("audio_left").unwrap() > 0.3, "{mode}");
        assert_eq!(read(&player, "playing"), true.into(), "{mode}");
        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }
}
