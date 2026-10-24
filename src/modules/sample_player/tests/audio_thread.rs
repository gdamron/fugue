//! Requests apply on the thread running the player, between blocks: a
//! retrigger is never lost to a one-shot's end.

use super::{read, write, write_test_wav};
use crate::modules::sample_player::{build, SamplePlayer};
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
