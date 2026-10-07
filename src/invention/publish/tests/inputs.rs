//! Input values across publications: a survivor's unconnected input keeps
//! the value last written to it, so an edit elsewhere leaves it
//! sample-identical to an unedited twin given the same write, while added
//! and rebuilt modules start fresh and connected inputs follow their
//! sources.

use super::carry::assert_identical;
use super::*;

/// `osc1` and a vca into the dac, behind an unrelated `lfo` that feeds
/// nothing audible. Nothing feeds `osc1`'s `frequency` or the vca's `audio`,
/// so [`written`] sets them directly. `lfo` comes first, so removing it
/// shifts every other module's index.
const HELD: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "lfo", "type": "oscillator", "config": { "frequency": 3.0 } },
        { "id": "osc1", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
        { "id": "vca", "type": "vca" },
        { "id": "dac", "type": "dac" }
    ],
    "connections": [
        { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio" },
        { "from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio" }
    ]
}"#;

/// The written frequency, audibly off `osc1`'s 440 Hz control.
const FREQUENCY: f32 = 660.0;
/// The written vca level, a constant offset in the mix.
const LEVEL: f32 = 0.25;

/// A [`HELD`] rig with `osc1`'s frequency and the vca's audio written and
/// applied.
fn written() -> Rig {
    let mut rig = Rig::new(HELD);
    rig.live
        .write_input("osc1", "frequency", FREQUENCY)
        .unwrap();
    rig.live.write_input("vca", "audio", LEVEL).unwrap();
    rig.render(1);
    rig
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

/// The whole input block of `id`'s `port`.
fn input_block(rig: &mut Rig, id: &str, port: &str) -> Vec<f32> {
    let module = rig.graph.modules.get_mut(id).unwrap().module_mut();
    let index = module.input_port_index(port).unwrap();
    module.input_block_mut(index).to_vec()
}

/// The last block `id` wrote to its first output port.
fn output_block(rig: &Rig, id: &str) -> Vec<f32> {
    let index = rig.graph.modules.get_index_of(id).unwrap();
    rig.graph.out_bufs[index][..64].to_vec()
}

/// Both written values are still in place and heard.
#[track_caller]
fn assert_held(rig: &mut Rig) {
    assert!(input_block(rig, "osc1", "frequency")
        .iter()
        .all(|v| *v == FREQUENCY));
    assert!(input_block(rig, "vca", "audio").iter().all(|v| *v == LEVEL));
    assert!(output_block(rig, "vca").iter().all(|v| *v == LEVEL));
}

#[test]
fn written_inputs_hold_across_unrelated_edits() {
    let mut edited = written();
    let mut twin = written();
    assert_identical(&edited.render(3), &twin.render(3), "before editing");

    upsert(&edited, "aux", "oscillator", serde_json::json!({}));
    assert_identical(&edited.render(3), &twin.render(3), "after adding aux");

    let lfo_to_aux = edge("lfo", "audio", "aux", "fm");
    edited.live.connect(lfo_to_aux.clone()).unwrap();
    assert_identical(&edited.render(3), &twin.render(3), "after connecting aux");
    edited.live.disconnect(lfo_to_aux).unwrap();
    assert_identical(
        &edited.render(3),
        &twin.render(3),
        "after disconnecting aux",
    );

    // Another port of the written module changes connectivity: inaudible
    // at osc1's default fm amount of zero, and the written port is kept.
    edited
        .live
        .connect(edge("lfo", "audio", "osc1", "fm"))
        .unwrap();
    assert_identical(&edited.render(3), &twin.render(3), "after connecting fm");

    upsert(
        &edited,
        "aux",
        "oscillator",
        serde_json::json!({ "frequency": 9.0 }),
    );
    assert_identical(&edited.render(3), &twin.render(3), "after rebuilding aux");

    // Every module moves down one index; osc1's fm port is disconnected.
    edited.live.remove_module("lfo").unwrap();
    assert_identical(&edited.render(3), &twin.render(3), "after removing lfo");
    assert_eq!(edited.module_ids(), ["osc1", "vca", "dac", "aux"]);
    assert_held(&mut edited);
}

#[test]
fn written_inputs_hold_across_folded_publications() {
    let mut edited = written();
    let mut twin = written();

    // Three publications before the next block, installed as one.
    upsert(&edited, "aux", "oscillator", serde_json::json!({}));
    edited.live.remove_module("lfo").unwrap();
    edited
        .live
        .connect(edge("aux", "audio", "osc1", "am"))
        .unwrap();
    assert_identical(&edited.render(5), &twin.render(5), "after folding");
    assert_held(&mut edited);
}

#[test]
fn rebuilt_and_added_modules_start_fresh() {
    let mut edited = written();
    let mut plain = Rig::new(HELD);
    plain.render(1);

    // Rebuild osc1 and remove the vca in both: neither written value
    // reaches the new graph, so it plays exactly as the unwritten one.
    for rig in [&edited, &plain] {
        let osc1 = rig.build(
            "osc1",
            "oscillator",
            serde_json::json!({ "waveform": "sine", "frequency": 440.0 }),
        );
        rig.live
            .edit(|change| {
                change.upsert("osc1", osc1);
                change.remove("vca");
                Ok(())
            })
            .unwrap();
    }
    assert_identical(&edited.render(3), &plain.render(3), "after rebuilding");

    // Adding the vca back builds a new one: its audio input is silent.
    for rig in [&edited, &plain] {
        upsert(rig, "vca", "vca", serde_json::json!({}));
        rig.live
            .connect(edge("vca", "audio", "dac", "audio"))
            .unwrap();
    }
    assert_identical(&edited.render(3), &plain.render(3), "after adding");
    assert!(input_block(&mut edited, "vca", "audio")
        .iter()
        .all(|v| *v == 0.0));
    assert!(input_block(&mut edited, "osc1", "frequency")
        .iter()
        .all(|v| *v == 0.0));
}

#[test]
fn a_connected_input_follows_its_source_and_resets_when_disconnected() {
    let mut rig = written();
    let osc1_to_vca = edge("osc1", "audio", "vca", "audio");

    // Connecting the written port hands it to its source.
    rig.live.connect(osc1_to_vca.clone()).unwrap();
    rig.render(1);
    let osc1 = output_block(&rig, "osc1");
    assert!(osc1.iter().any(|v| *v != 0.0));
    assert_eq!(output_block(&rig, "vca"), osc1);

    // Disconnecting it clears the routed samples from the whole buffer
    // rather than holding the last block; the write was superseded.
    rig.live.disconnect(osc1_to_vca).unwrap();
    rig.graph.ensure_process_order();
    assert!(input_block(&mut rig, "vca", "audio")
        .iter()
        .all(|v| *v == 0.0));
    rig.render(1);
    assert!(output_block(&rig, "vca").iter().all(|v| *v == 0.0));

    // The other write, on a port that stayed unconnected, held throughout.
    assert!(input_block(&mut rig, "osc1", "frequency")
        .iter()
        .all(|v| *v == FREQUENCY));
}

#[test]
fn written_inputs_hold_when_an_install_falls_back_to_recompiling() {
    let mut edited = written();
    let mut twin = written();

    // A block size change recompiles in place, keeping both writes.
    for rig in [&mut edited, &mut twin] {
        rig.graph.set_block_size(128);
    }
    assert_identical(&edited.render(3), &twin.render(3), "after resizing");
    assert_eq!(edited.graph.block_capacity, 128);
    assert_held(&mut edited);

    // Publications are still compiled for the linked block size, so each
    // install now falls back to recompiling from the installed instances.
    assert_eq!(edited.live.publisher().lock().unwrap().block_size(), 64);
    upsert(&edited, "aux", "oscillator", serde_json::json!({}));
    assert_identical(&edited.render(3), &twin.render(3), "after adding aux");
    edited.live.remove_module("lfo").unwrap();
    assert_identical(&edited.render(3), &twin.render(3), "after removing lfo");
    assert_eq!(edited.graph.block_capacity, 128);
    assert_held(&mut edited);
}
