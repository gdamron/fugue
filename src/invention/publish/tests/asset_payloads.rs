//! Asset controls on a live graph: a write loads its sample at the engine
//! rate on the control thread, the audio thread adopts it without
//! allocating, the handle it replaces is released by the reclaimer (the
//! sample cache keeps the buffer), and a load that fails refuses the write
//! with its reason, sending nothing.

use std::thread;

use super::migrated::{read, write};
use super::requests::outcomes;
use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::modules::sample_loading::load_cached_sample;
use crate::test_support::sampler::{wav, SamplerFactory, SAMPLER};
use crate::ControlValue;

const DAC: &str = r#"{
    "version": "1.0.0",
    "modules": [{ "id": "dac", "type": "dac" }],
    "connections": []
}"#;

/// A sampler holding `samples`, live.
fn rig(samples: [&str; 2]) -> Rig {
    let mut rig = Rig::new(DAC);
    rig.registry.register(SamplerFactory);
    rig.adopt_registry();
    let sampler = rig.build("kit", SAMPLER, serde_json::json!({ "samples": samples }));
    rig.live
        .edit(|change| {
            change.upsert("kit", sampler);
            Ok(())
        })
        .unwrap();
    rig.render(1);
    rig.live.reclaim();
    rig
}

/// Slot `slot`'s sample length, in engine-rate frames.
fn frames(rig: &Rig, slot: usize) -> f32 {
    rig.graph.modules["kit"].module().output_block(slot)[0]
}

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
fn an_asset_write_loads_at_the_engine_rate_and_applies_without_allocating() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 441), wav(dir.path(), "b.wav", 441));
    let c = wav(dir.path(), "c.wav", 882);
    let mut rig = rig([&a, &b]);
    assert_eq!((frames(&rig, 0), frames(&rig, 1)), (480.0, 480.0));
    let replaced = load_cached_sample(&b, SAMPLE_RATE).unwrap();
    let held = Arc::strong_count(&replaced);

    write(&rig, "kit", "asset.1", c.as_str().into());
    assert_eq!(
        read(&rig, "kit", "asset.1"),
        c.as_str().into(),
        "as written"
    );
    assert_eq!(frames(&rig, 1), 480.0, "not before the next block");
    assert_eq!(audio_block(&mut rig), (0, 0));
    assert_eq!(
        frames(&rig, 1),
        960.0,
        "441 frames at 44.1 kHz, twice, at 48"
    );
    assert_eq!(frames(&rig, 0), 480.0, "only the slot written");

    // The kit's handle on the old sample is retired, not dropped, on the
    // audio thread; the reclaimer releases it here, and the cache keeps
    // the buffer.
    assert_eq!(Arc::strong_count(&replaced), held);
    rig.live.reclaim();
    assert_eq!(Arc::strong_count(&replaced), held - 1);
    assert_eq!(replaced.len(), 480);
}

#[test]
fn an_asset_that_does_not_load_is_refused_as_written_and_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 441), wav(dir.path(), "b.wav", 441));
    let mut rig = rig([&a, &b]);
    let surface = rig.live.control_surfaces.lock().unwrap()["kit"].clone();
    let surfaces = rig.live.control_surfaces.lock().unwrap().clone();
    let missing = dir.path().join("missing.wav");
    let missing = missing.to_string_lossy();

    // Validating checks the name alone: whether it loads is known by
    // loading it, which the write does (an edit batch reports it there).
    let number = ControlValue::Number(1.0);
    let refused = surface.validate_control("asset.0", &number, &surfaces);
    assert_eq!(surface.set_control("asset.0", number), refused);
    assert!(refused.is_err());
    let named = ControlValue::from(missing.as_ref());
    assert_eq!(
        surface.validate_control("asset.0", &named, &surfaces),
        Ok(())
    );
    let error = surface.set_control("asset.0", named).unwrap_err();
    assert!(error.contains("missing.wav"), "names the file: {error}");

    assert_eq!(audio_block(&mut rig), (0, 0));
    assert!(outcomes(&mut rig).is_empty(), "no request was sent");
    assert_eq!(read(&rig, "kit", "asset.0"), a.as_str().into());
    assert_eq!(frames(&rig, 0), 480.0);
}
