//! Allocation-counted publishing: edits queued before a block, a full retire
//! channel, and input writes beside, across, and waiting for a publication.
//! Each counts the install (`ensure_process_order`) and the blocks after it.
//! The shapes a single change can take are counted where changes are
//! prepared.

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
fn edits_queued_before_a_block_install_in_it_without_recompiling() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    upsert(&rig, "osc3", "oscillator", serde_json::json!({}));
    rig.live
        .connect(edge("osc3", "audio", "dac", "audio"))
        .unwrap();
    assert_eq!(rig.generation_and_applied(), (2, 0));

    assert_clean_install(&mut rig, "two queued edits");
    assert_eq!(rig.generation_and_applied(), (2, 2));
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "osc3"]);
    let dac = rig.graph.modules.get_index_of("dac").unwrap();
    assert_eq!(rig.graph.compiled_routes[dac].len(), 3);
}

#[test]
fn a_full_retire_channel_keeps_blocks_clean_until_drained() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    let fm = edge("osc1", "audio", "osc2", "frequency_mod");
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

/// The value in the first frame of `id`'s `port` input block.
fn input_value(rig: &mut Rig, id: &str, port: &str) -> f32 {
    let module = rig.graph.modules.get_mut(id).unwrap().module_mut();
    let index = module.input_port_index(port).unwrap();
    module.input_block_mut(index)[0]
}

#[test]
fn an_input_write_beside_a_publication_is_clean() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    upsert(&rig, "osc3", "oscillator", serde_json::json!({}));
    rig.live.write_input("osc1", "frequency", 0.5).unwrap();

    // The write is a plain record resolved on the control thread: nothing
    // to free when it is applied.
    let ((), allocs, frees) = allocator_events(|| rig.graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0));
    assert!(!rig.graph.topo_dirty);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "osc3"]);
    assert_eq!(input_value(&mut rig, "osc1", "frequency"), 0.5);
}

#[test]
fn a_write_remapped_across_an_install_is_clean() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    // Resolved while osc2 is at index 1; removing osc1 moves it to 0.
    rig.live.write_input("osc2", "frequency", 0.5).unwrap();
    rig.live.remove_module("osc1").unwrap();

    let ((), allocs, frees) = allocator_events(|| rig.graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0));
    assert!(!rig.graph.topo_dirty);
    assert_eq!(rig.module_ids(), ["osc2", "dac"]);
    assert_eq!(input_value(&mut rig, "osc2", "frequency"), 0.5);
}

#[test]
fn a_write_between_two_queued_edits_is_clean() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    upsert(&rig, "osc3", "oscillator", serde_json::json!({}));
    rig.live.write_input("osc3", "frequency", 0.5).unwrap();
    // Queued behind the edit that adds osc3, before the block installs both.
    rig.live.remove_module("osc1").unwrap();

    let ((), allocs, frees) = allocator_events(|| rig.graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0));
    assert!(!rig.graph.topo_dirty);
    assert_eq!(rig.module_ids(), ["osc2", "dac", "osc3"]);
    assert_eq!(input_value(&mut rig, "osc3", "frequency"), 0.5);
}

#[test]
fn a_write_held_for_a_pending_publication_is_clean() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    rig.hold_a_retirement();
    let osc3 = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc3));
    rig.live.write_input("osc3", "frequency", 0.5).unwrap();

    // The publication stays untaken, so the write waits in the ring.
    let ((), allocs, frees) = allocator_events(|| rig.graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0), "holding the write");
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac"]);

    // Once there is room the publication installs and the write follows.
    rig.live.reclaim();
    let ((), allocs, frees) = allocator_events(|| rig.graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0), "applying the held write");
    assert!(!rig.graph.topo_dirty);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "osc3"]);
    assert_eq!(input_value(&mut rig, "osc3", "frequency"), 0.5);
}

#[test]
fn installs_that_keep_written_inputs_stay_clean() {
    let mut rig = Rig::new(BASE);
    rig.live.write_input("osc1", "frequency", 0.5).unwrap();
    rig.live.write_input("osc2", "frequency", 0.75).unwrap();
    rig.render(1);
    let osc3_to_osc2 = edge("osc3", "audio", "osc2", "frequency");

    // Each install compares every survivor's connectivity before and after
    // and keeps the written ports that stay unconnected.
    upsert(&rig, "osc3", "oscillator", serde_json::json!({}));
    assert_clean_install(&mut rig, "adding a module");
    rig.live
        .connect(edge("osc1", "audio", "osc2", "frequency_mod"))
        .unwrap();
    assert_clean_install(&mut rig, "connecting another port");
    rig.live.connect(osc3_to_osc2.clone()).unwrap();
    assert_clean_install(&mut rig, "connecting a written port");
    rig.live.disconnect(osc3_to_osc2).unwrap();
    assert_clean_install(&mut rig, "disconnecting it");
    assert_eq!(input_value(&mut rig, "osc1", "frequency"), 0.5);
    assert_eq!(input_value(&mut rig, "osc2", "frequency"), 0.0);

    // Removing osc1 moves osc2, written again, to a new index.
    rig.live.write_input("osc2", "frequency", 0.75).unwrap();
    rig.render(1);
    rig.live.remove_module("osc1").unwrap();
    assert_clean_install(&mut rig, "removing a module");
    assert_eq!(rig.module_ids(), ["osc2", "dac", "osc3"]);
    assert_eq!(input_value(&mut rig, "osc2", "frequency"), 0.75);

    upsert(&rig, "osc3", "oscillator", serde_json::json!({}));
    assert_clean_install(&mut rig, "rebuilding a module");
    assert_eq!(input_value(&mut rig, "osc2", "frequency"), 0.75);
}

#[test]
fn an_install_that_carries_feedback_state_stays_clean() {
    let mut rig = Rig::new(super::carry::LOOP);
    rig.render(3);
    let loop_ids = ["osc1", "osc2"];
    let carries = |rig: &Rig| -> Vec<Vec<f32>> {
        loop_ids
            .iter()
            .map(|id| rig.graph.out_prev[rig.graph.modules.get_index_of(*id).unwrap()].clone())
            .collect()
    };
    let before = carries(&rig);
    assert!(before.iter().flatten().any(|v| *v != 0.0));

    // Removing the lfo shifts both loop modules, so each carry is copied
    // to a new index during the counted install.
    rig.live.remove_module("lfo").unwrap();
    let ((), allocs, frees) = allocator_events(|| rig.graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0), "install that carries");
    assert!(!rig.graph.topo_dirty);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac"]);
    assert_eq!(carries(&rig), before);

    // Two edits queued before a block install in order, each carrying the
    // loop through its remap, just as cleanly.
    upsert(&rig, "aux", "oscillator", serde_json::json!({}));
    rig.live
        .connect(edge("aux", "audio", "osc2", "amplitude_mod"))
        .unwrap();
    let ((), allocs, frees) = allocator_events(|| rig.graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0), "two installs that carry");
    assert!(!rig.graph.topo_dirty);
    assert_eq!(carries(&rig), before);
}
