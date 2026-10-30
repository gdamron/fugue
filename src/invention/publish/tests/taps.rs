//! Output taps on a live graph: they follow their module across edits,
//! close as it is removed or rebuilt, and change nothing anyone hears.

use super::*;
use crate::OutputBins;

/// Every read's bins, checked contiguous and open.
fn read_all(tap: &mut OutputTap, reads: &mut Vec<OutputBins>) {
    let read = tap.read(32);
    assert!(read.open, "{read:?}");
    if let Some(last) = reads.iter().rev().find(|read| !read.is_empty()) {
        if !read.is_empty() {
            let end = last.origin_frame + (last.len() as u64 * 32);
            assert_eq!((read.origin_frame, read.lost), (end, 0));
        }
    }
    reads.push(read);
}

#[test]
fn opening_and_closing_taps_leaves_the_output_sample_identical() {
    let render = |open: bool| {
        let mut rig = Rig::new(BASE);
        let mut out = Vec::new();
        let mut taps = Vec::new();
        for block in 0..300 {
            if open && block % 60 == 10 {
                taps.push(rig.live.tap_output("osc1", &[]).unwrap());
                taps.push(rig.live.tap_output("dac", &[]).unwrap());
            }
            if open && block % 60 == 40 {
                taps.clear();
            }
            for tap in &mut taps {
                tap.read(64);
            }
            out.extend(rig.render(1).iter().map(|sample| sample.to_bits()));
        }
        out
    };
    let heard = render(false);
    assert!(heard.iter().any(|&bits| f32::from_bits(bits) != 0.0));
    assert_eq!(render(true), heard);
}

#[test]
fn a_tap_follows_its_module_as_edits_move_it() {
    let mut rig = Rig::new(BASE);
    let mut tap = rig.live.tap_output("osc2", &[]).unwrap();
    assert_eq!(tap.ports(), ["audio"]);
    let mut reads = Vec::new();
    rig.render(20);
    read_all(&mut tap, &mut reads);
    // Removing the module before it moves it to another index.
    rig.live
        .edit(|change| {
            change.remove("osc1");
            Ok(())
        })
        .unwrap();
    for _ in 0..4 {
        rig.render(20);
        read_all(&mut tap, &mut reads);
    }
    assert_eq!(rig.module_ids(), ["osc2", "dac"]);
    let bins: usize = reads.iter().map(OutputBins::len).sum();
    assert_eq!(bins, 100 * 64 / 32, "every sample, in one contiguous run");
    let peak = (reads.iter().flat_map(|read| &read.channels[0].max)).fold(0.0f32, |a, &b| a.max(b));
    assert!(
        (0.9..=1.001).contains(&peak),
        "a sine at full scale: {peak}"
    );
}

#[test]
fn removing_or_rebuilding_the_module_closes_its_tap() {
    let mut rig = Rig::new(BASE);
    let mut rebuilt = rig.live.tap_output("osc1", &[]).unwrap();
    let mut removed = rig.live.tap_output("osc2", &[]).unwrap();
    let mut dac = rig.live.tap_output("dac", &[]).unwrap();
    rig.render(4);
    let square = rig.build(
        "osc1",
        "oscillator",
        serde_json::json!({ "waveform": "square" }),
    );
    rig.live
        .edit(|change| {
            change.upsert("osc1", square);
            Ok(())
        })
        .unwrap();
    rig.render(4);
    assert!(
        !rebuilt.read(32).open,
        "a rebuilt module is another instance"
    );
    assert!(removed.read(32).open);
    rig.live
        .edit(|change| {
            change.remove("osc2");
            Ok(())
        })
        .unwrap();
    rig.render(4);
    assert!(!removed.read(32).open);
    let dac = dac.read(32);
    assert!(dac.open && dac.len() == 12 * 64 / 32, "{dac:?}");
    let heard = dac.channels.iter().flat_map(|channel| &channel.max);
    assert!(
        heard.fold(0.0f32, |a, &b| a.max(b)) > 0.1,
        "the dac's own output"
    );
    // The new module under the old id can be tapped again.
    let mut again = rig.live.tap_output("osc1", &[]).unwrap();
    rig.render(2);
    assert_eq!(again.read(32).len(), 4);
    // Every tap closes as the graph goes.
    drop(rig);
    assert!(!again.read(32).open);
}
