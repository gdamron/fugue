//! A sample player on a live graph: an asset write swaps its sample during
//! playback without the audio thread allocating or freeing (the reclaimer
//! releases the handle it replaces), and every `play` request applies.

use std::thread;

use super::migrated::{read, write};
use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::modules::sample_loading::load_cached_sample;
use crate::test_support::sampler::wav;

const DAC: &str = r#"{
    "version": "1.0.0",
    "modules": [{ "id": "dac", "type": "dac" }],
    "connections": []
}"#;

/// A sample player built from `config`, live.
fn rig(config: serde_json::Value) -> Rig {
    let mut rig = Rig::new(DAC);
    let player = rig.build("player", "sample_player", config);
    rig.live
        .edit(|change| {
            change.upsert("player", player);
            Ok(())
        })
        .unwrap();
    rig.render(1);
    rig.live.reclaim();
    rig
}

/// The first frame of the last block's output `port`.
fn first(rig: &Rig, port: usize) -> f32 {
    rig.graph.modules["player"].module().output_block(port)[0]
}

const START: usize = 2;

/// Renders one block on a thread standing in for the audio thread, inside
/// its scope (where a payload dropping panics in debug builds), counting
/// its allocations and frees.
fn audio_block(rig: &mut Rig) -> (usize, usize) {
    let graph = &mut rig.graph;
    thread::scope(|scope| {
        scope
            .spawn(|| {
                let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
                let ((), allocs, frees) = allocator_events(|| {
                    let _audio = AudioThreadScope::enter();
                    graph.process_block(&mut left, &mut right);
                });
                (allocs, frees)
            })
            .join()
            .unwrap()
    })
}

#[test]
fn an_asset_swap_during_playback_applies_without_allocating_and_is_reclaimed() {
    for mode in ["classic", "elastic"] {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (
            wav(dir.path(), "a.wav", 4410),
            wav(dir.path(), "b.wav", 441),
        );
        let mut rig = rig(serde_json::json!({
            "asset": a, "mode": mode, "autoplay": true, "loop": true,
        }));
        let replaced = load_cached_sample(&a, SAMPLE_RATE).unwrap();
        let held = Arc::strong_count(&replaced);

        write(&rig, "player", "asset", b.as_str().into());
        assert_eq!(read(&rig, "player", "asset"), b.as_str().into());
        assert_eq!(audio_block(&mut rig), (0, 0), "{mode}");
        assert_eq!(first(&rig, START), 1.0, "{mode}: the new sample starts");
        assert_eq!(read(&rig, "player", "playing"), true.into(), "{mode}");

        assert_eq!(Arc::strong_count(&replaced), held, "{mode}");
        rig.live.reclaim();
        assert_eq!(Arc::strong_count(&replaced), held - 1, "{mode}");
    }
}

#[test]
fn every_play_request_applies_live() {
    let dir = tempfile::tempdir().unwrap();
    let a = wav(dir.path(), "a.wav", 4410);
    let mut rig = rig(serde_json::json!({ "asset": a }));
    assert_eq!(read(&rig, "player", "playing"), false.into());

    for _ in 0..3 {
        write(&rig, "player", "play", true.into());
        assert_eq!(audio_block(&mut rig), (0, 0));
        assert_eq!(first(&rig, START), 1.0, "each start restarts");
        assert_eq!(read(&rig, "player", "playing"), true.into());
    }
    write(&rig, "player", "play", false.into());
    assert_eq!(audio_block(&mut rig), (0, 0));
    assert_eq!(read(&rig, "player", "playing"), false.into());
}
