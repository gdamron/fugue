//! A live write or edit loads under the lock its graph was built under, not
//! the session's slot, which a host moves as a reload commits rather than in
//! the same step (the fifth Codex round on the frozen-lock fix).

use super::bytes::{playable_kit, sampler};
use super::*;
use crate::invention::manual_backend::Settled;
use crate::invention::InventionBuilder;
use crate::rpc::StructuralEdit;
use crate::ControlValue;

/// Runs `body` on a graph built under `playable_kit`'s lock (1.0.0 locked,
/// a tampered 1.1.0 beside it), with this thread's slot ungoverned, as it
/// is while a reload under no lock commits or a timed one waits to sync.
fn under_a_moved_slot(body: impl FnOnce(&mut Settled, &str)) {
    let tmp = tempfile::tempdir().unwrap();
    let (packs, lock) = playable_kit(tmp.path());
    with_packs_dir(&packs, || {
        with_governing(lock, || {
            let (runtime, _) = InventionBuilder::new(48_000)
                .build(sampler(&format!("{KIT}@1.0.0:a.wav")))
                .unwrap();
            let mut running = Settled::start(runtime);
            govern_here(None, || body(&mut running, &format!("{KIT}@^1.1:a.wav")));
        });
    });
}

#[test]
fn a_live_write_loads_under_the_lock_its_graph_was_built_under() {
    under_a_moved_slot(|running, newer| {
        let err = running
            .set_control("s", "asset", ControlValue::String(newer.into()))
            .unwrap_err();
        assert!(err.to_string().contains("does not satisfy"), "{err}");
    });
}

#[test]
fn a_live_edit_builds_under_the_lock_of_the_graph_it_edits() {
    under_a_moved_slot(|running, newer| {
        let config = json!({ "asset": newer });
        let err = running.add_module("t", "sample_player", &config).err();
        let err = err.expect("the add is refused").to_string();
        assert!(err.contains("does not satisfy"), "{err}");

        let err = running
            .apply_edits(&[StructuralEdit::AddModule {
                id: "u".into(),
                module_type: "sample_player".into(),
                config,
            }])
            .unwrap_err();
        assert!(format!("{err:?}").contains("does not satisfy"), "{err:?}");
    });
}
