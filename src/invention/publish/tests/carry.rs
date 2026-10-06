//! Feedback-loop carries across publications: a survivor's previous-block
//! outputs (`out_prev`) carry over, so an edit that does not touch a loop
//! leaves it sample-identical to an unedited twin, while a rebuilt module
//! starts from zero.

use super::*;

/// A sustained FM feedback loop (`osc1` <-> `osc2`) into the dac, behind an
/// unrelated `lfo` that feeds nothing audible. `lfo` comes first, so
/// removing it shifts every loop module's index.
pub(super) const LOOP: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "lfo", "type": "oscillator", "config": { "frequency": 3.0 } },
        { "id": "osc1", "type": "oscillator", "config": { "frequency": 220.0, "fm_amount": 300.0 } },
        { "id": "osc2", "type": "oscillator", "config": { "frequency": 331.0, "fm_amount": 500.0 } },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc1", "from_port": "audio", "to": "osc2", "to_port": "fm" },
        { "from": "osc2", "from_port": "audio", "to": "osc1", "to_port": "fm" },
        { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// Upserts a fresh oscillator as `id` (adding it, or rebuilding it in place).
fn upsert_osc(rig: &Rig, id: &str, config: serde_json::Value) {
    let built = rig.build(id, "oscillator", config);
    rig.live
        .edit(|change| {
            change.upsert(id, built);
            Ok(())
        })
        .unwrap();
}

/// Asserts two renders are bit-identical, naming the first sample that
/// differs rather than printing both renders.
#[track_caller]
fn assert_identical(edited: &[f32], twin: &[f32], label: &str) {
    assert_eq!(edited.len(), twin.len(), "{label}: lengths differ");
    if let Some(i) = (0..edited.len()).find(|&i| edited[i].to_bits() != twin[i].to_bits()) {
        panic!(
            "{label}: sample {i} differs (block {}, frame {}): {} vs {}",
            i / 64,
            i % 64,
            edited[i],
            twin[i]
        );
    }
}

/// The carry stored for module `id`'s output ports.
fn carry(rig: &Rig, id: &str) -> Vec<f32> {
    rig.graph.out_prev[rig.graph.modules.get_index_of(id).unwrap()].clone()
}

#[test]
fn an_edit_outside_a_feedback_loop_leaves_it_sample_identical() {
    let mut edited = Rig::new(LOOP);
    let mut twin = Rig::new(LOOP);
    assert_identical(&edited.render(5), &twin.render(5), "before editing");

    // Add a module and wire the lfo into it: the loop is untouched.
    upsert_osc(&edited, "aux", serde_json::json!({ "frequency": 90.0 }));
    edited
        .live
        .connect(edge("lfo", "audio", "aux", "fm"))
        .unwrap();
    assert_identical(&edited.render(7), &twin.render(7), "after adding aux");

    // Remove the lfo: every loop module moves down one index.
    edited.live.remove_module("lfo").unwrap();
    assert_identical(&edited.render(7), &twin.render(7), "after removing lfo");

    // Rebuild a module outside the loop.
    upsert_osc(&edited, "aux", serde_json::json!({ "frequency": 45.0 }));
    assert_identical(&edited.render(7), &twin.render(7), "after rebuilding aux");
    assert_eq!(edited.generation_and_applied(), (4, 3));
}

#[test]
fn folded_publications_keep_the_loop_sample_identical() {
    let mut edited = Rig::new(LOOP);
    let mut twin = Rig::new(LOOP);
    assert_identical(&edited.render(3), &twin.render(3), "before editing");

    // Four publications before the next block: the audio thread installs
    // one, folded together, against the order it is still running.
    upsert_osc(&edited, "aux", serde_json::json!({}));
    edited.live.remove_module("lfo").unwrap();
    edited
        .live
        .connect(edge("aux", "audio", "osc2", "am"))
        .unwrap();
    edited
        .live
        .disconnect(edge("aux", "audio", "osc2", "am"))
        .unwrap();
    assert_eq!(edited.generation_and_applied(), (4, 0));

    assert_identical(&edited.render(10), &twin.render(10), "after folding");
    assert_eq!(edited.generation_and_applied(), (4, 1));
}

#[test]
fn a_rebuilt_feedback_loop_starts_from_zero() {
    let mut edited = Rig::new(LOOP);
    let mut fresh = Rig::new(LOOP);
    edited.render(5);
    assert!(carry(&edited, "osc2").iter().any(|v| *v != 0.0));

    // Rebuild both loop modules as written: the loop starts over exactly as
    // a freshly built graph does, carries at zero.
    let osc1 = edited.build(
        "osc1",
        "oscillator",
        serde_json::json!({ "frequency": 220.0, "fm_amount": 300.0 }),
    );
    let osc2 = edited.build(
        "osc2",
        "oscillator",
        serde_json::json!({ "frequency": 331.0, "fm_amount": 500.0 }),
    );
    edited
        .live
        .edit(|change| {
            change.upsert("osc1", osc1);
            change.upsert("osc2", osc2);
            Ok(())
        })
        .unwrap();
    edited.graph.ensure_process_order();
    assert!(carry(&edited, "osc1").iter().all(|v| *v == 0.0));
    assert!(carry(&edited, "osc2").iter().all(|v| *v == 0.0));
    assert_identical(&edited.render(10), &fresh.render(10), "after rebuilding");
}

#[test]
fn only_the_rebuilt_half_of_a_loop_loses_its_carry() {
    let mut rig = Rig::new(LOOP);
    rig.render(5);
    let osc2_before = carry(&rig, "osc2");
    assert!(osc2_before.iter().any(|v| *v != 0.0));

    upsert_osc(&rig, "osc1", serde_json::json!({ "frequency": 220.0 }));
    rig.graph.ensure_process_order();
    assert!(carry(&rig, "osc1").iter().all(|v| *v == 0.0));
    assert_eq!(carry(&rig, "osc2"), osc2_before);
}

#[test]
fn a_module_rebuilt_by_a_folded_publication_starts_from_zero() {
    let mut rig = Rig::new(LOOP);
    rig.render(5);
    let osc2_before = carry(&rig, "osc2");

    // The first publication rebuilds osc1; the second, folded into it
    // before any block, treats that new osc1 as a survivor. Its carry must
    // still start from zero: the running osc1 never reaches the new graph.
    upsert_osc(&rig, "osc1", serde_json::json!({ "frequency": 220.0 }));
    upsert_osc(&rig, "aux", serde_json::json!({}));
    assert_eq!(rig.generation_and_applied(), (2, 0));
    rig.graph.ensure_process_order();
    assert_eq!(rig.generation_and_applied(), (2, 1));
    assert!(carry(&rig, "osc1").iter().all(|v| *v == 0.0));
    assert_eq!(carry(&rig, "osc2"), osc2_before);
}
