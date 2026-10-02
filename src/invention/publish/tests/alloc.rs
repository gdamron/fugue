//! Allocation-counted publishing: folded publications, a full retire
//! channel, and input writes beside a publication. Each counts the install
//! (`ensure_process_order`) and the blocks after it. The shapes a single
//! change can take are counted where changes are prepared.

use super::*;
use crate::invention::publish::publisher::RETIRE_CAPACITY;

/// Counts one install and two blocks after a commit, which must neither
/// allocate nor free nor fall back to recompiling.
fn assert_clean_install(rig: &mut Rig, label: &str) {
    let ((), allocs, frees) = allocator_events(|| rig.graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0), "{label}: install");
    assert!(!rig.graph.topo_dirty, "{label}: fell back to recompiling");
    let mut left = [0.0f32; 64];
    let mut right = [0.0f32; 64];
    for block in 0..2 {
        let ((), allocs, frees) =
            allocator_events(|| rig.graph.process_block(&mut left, &mut right));
        assert_eq!((allocs, frees), (0, 0), "{label}: block {block}");
    }
}

fn upsert(rig: &Rig, id: &str, module_type: &str, config: serde_json::Value) {
    let built = rig.build(id, module_type, config);
    rig.live
        .edit(|change| {
            change.upsert(id, built);
            Ok(())
        })
        .unwrap();
}

#[test]
fn folded_publications_install_once_without_recompiling() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    upsert(&rig, "osc3", "oscillator", serde_json::json!({}));
    rig.live
        .connect(edge("osc3", "audio", "dac", "audio"))
        .unwrap();
    assert_eq!(rig.generation_and_applied(), (2, 0));

    assert_clean_install(&mut rig, "two folded publications");
    assert_eq!(rig.generation_and_applied(), (2, 1));
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "osc3"]);
    let dac = rig.graph.modules.get_index_of("dac").unwrap();
    assert_eq!(rig.graph.compiled_routes[dac].len(), 3);
}

#[test]
fn a_full_retire_channel_keeps_blocks_clean_until_drained() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    let fm = edge("osc1", "audio", "osc2", "fm");
    // Publishes without `begin`, which would reclaim and empty the channel.
    let toggle = |rig: &Rig, n: usize| {
        let mut publisher = rig.live.publisher().lock().unwrap();
        let mut change = rig.live.change_on(&publisher);
        if n.is_multiple_of(2) {
            change.connect(fm.clone()).unwrap();
        } else {
            change.disconnect(fm.clone());
        }
        publisher.publish(change.prepare().unwrap()).unwrap();
    };

    // Fill the channel, then one more retirement is held.
    for n in 0..=RETIRE_CAPACITY {
        toggle(&rig, n);
        assert_clean_install(&mut rig, "retire into a filling channel");
    }
    let applied = rig.generation_and_applied().1;

    // While one is held, nothing more is taken and blocks stay clean.
    toggle(&rig, RETIRE_CAPACITY + 1);
    assert_clean_install(&mut rig, "publication waiting on a full channel");
    assert_eq!(rig.generation_and_applied().1, applied);

    assert_eq!(rig.live.reclaim(), RETIRE_CAPACITY);
    assert_clean_install(&mut rig, "drained channel");
    assert_eq!(rig.generation_and_applied().1, applied + 1);
}

#[test]
fn an_input_write_beside_a_publication_frees_only_its_strings() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    upsert(&rig, "osc3", "oscillator", serde_json::json!({}));
    rig.live
        .write_input(InputWrite {
            module_id: "osc1".to_string(),
            port: "fm".to_string(),
            value: 0.5,
        })
        .unwrap();

    // The install itself is clean; the write's two id strings are dropped
    // on the audio thread. This count drops to zero once input writes are
    // resolved on the control thread to plain indices.
    let ((), allocs, frees) = allocator_events(|| rig.graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 2));
    assert!(!rig.graph.topo_dirty);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "osc3"]);
}
