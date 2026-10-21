//! Tables writing one control at one sample: the later module's entry
//! lands, whichever table the drain found first.

use super::*;

/// A dial, and tablers `ta` then `tb` (in module order) writing its level
/// at one sample: `ta` 0.5 at step `ta_step`, `tb` 0.75 at step 2. No
/// clock reaches either yet.
fn rivals(ta_step: u64) -> (Rig, Automation) {
    let mut rig = Rig::new(DOCUMENT);
    rig.registry.register(DialFactory);
    rig.registry.register(TablerFactory);
    let dial = rig.build("dial", DIAL, serde_json::json!({}));
    let level = dial.surface.as_ref().unwrap().automation("level").unwrap();
    let ta = rival(&rig, "ta", 1, ta_step, 0.5, &level);
    let tb = rival(&rig, "tb", 2, 2, 0.75, &level);
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.upsert("ta", ta);
            change.upsert("tb", tb);
            change.connect(edge("dial", "out", "dac", "audio"))
        })
        .unwrap();
    (rig, level)
}

fn rival(
    rig: &Rig,
    id: &str,
    table: usize,
    step: u64,
    value: f32,
    level: &Automation,
) -> change::BuiltModule {
    next_table(TestTable::new(table, vec![entry(step, 0, value, level)]));
    rig.build(id, TABLER, serde_json::json!({}))
}

fn clock_to(rig: &mut Rig, to: &str) {
    rig.live
        .edit(|change| change.connect(edge("clock", "beat", to, "clock")))
        .unwrap();
}

/// Renders through beat 3 after `before`, checking `tb`, the later
/// module, landed at beat 2.
fn assert_tb_lands(rig: &mut Rig, before: Vec<f32>) {
    let beats = beat_samples(3);
    let after = render_counted(rig, blocks(3.0) - before.len() / 64);
    let out: Vec<f32> = before.into_iter().chain(after).collect();
    assert_eq!(out[beats[2] - 1], 0.25);
    assert_eq!(out[beats[2]], 0.75, "the later module's write lands");
    let index = |id| rig.graph.modules.get_index_of(id);
    assert!(index("ta") < index("tb"));
}

#[test]
fn tables_writing_at_one_sample_land_in_module_order_whenever_found() {
    // tb is found first, on beat 0; ta a block later, from beat 1, so its
    // step 1 is tb's step 2.
    let (mut rig, _) = rivals(1);
    clock_to(&mut rig, "tb");
    let before = render_counted(&mut rig, 1);
    clock_to(&mut rig, "ta");
    assert_tb_lands(&mut rig, before);
}

#[test]
fn tables_writing_at_one_sample_land_in_module_order_after_a_rebuild() {
    let (mut rig, level) = rivals(2);
    clock_to(&mut rig, "ta");
    clock_to(&mut rig, "tb");
    let before = render_counted(&mut rig, 1);
    // ta rebuilt in place is found anew, from beat 1.
    let ta = rival(&rig, "ta", 3, 1, 0.5, &level);
    rig.live
        .edit(|change| {
            change.upsert("ta", ta);
            Ok(())
        })
        .unwrap();
    assert_tb_lands(&mut rig, before);
}
