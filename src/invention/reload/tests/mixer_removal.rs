//! A reload that takes a channel away from a playing mixer fades it out,
//! whether it lowers the channel count or removes the module feeding it,
//! rather than stopping it at once.

use serde_json::json;

use super::mixer_rebuild::{document, start, steepest, BLOCKS_AFTER, BLOCKS_BEFORE, SLOPE_MARGIN};
use super::*;
use crate::invention::manual_backend::SAMPLE_RATE;
use crate::modules::mixer::gain_ramp_frames;

/// `document` without the tone feeding the mixer's second channel, or its
/// connection.
fn without_high(document: Invention) -> Invention {
    let mut json: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
    for (list, field) in [("modules", "id"), ("connections", "from")] {
        json[list]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry[field] != "high");
    }
    Invention::from_json(&json.to_string()).unwrap()
}

/// Plays `from`, reloads to `to` mid-note, and checks the output across
/// the reload is no steeper than the tones are on their own, before it or
/// once the fade is over: no step, only the glide's own slope. Returns the
/// modules the reload rebuilt.
fn a_removal_does_not_step(from: Invention, to: Invention) -> Vec<String> {
    removals_do_not_step(from, &[to])
}

/// [`a_removal_does_not_step`] for reloads to each of `to` in turn, all
/// before the next block, so they install together. Returns the modules
/// the last one rebuilt.
fn removals_do_not_step(from: Invention, to: &[Invention]) -> Vec<String> {
    let (mut running, pump) = start(from);
    let before = pump.render(BLOCKS_BEFORE);
    let mut report = None;
    for to in to {
        report = Some(running.reload(to.clone()).expect("the reload applies"));
    }
    let report = report.expect("a reload");
    let after = pump.render(BLOCKS_AFTER);

    let mut across = before[before.len() - 1..].to_vec();
    across.extend_from_slice(&after);
    let settled = &after[gain_ramp_frames(SAMPLE_RATE) as usize * 2..];
    let limit = steepest(&before).max(steepest(settled)) * SLOPE_MARGIN;
    assert!(
        steepest(&across) <= limit,
        "steepest {} over {limit}",
        steepest(&across)
    );
    report.swapped
}

#[test]
fn a_channel_a_lower_channel_count_drops_fades_out() {
    let swapped = a_removal_does_not_step(
        document(json!({ "channel_count": 2 })),
        document(json!({ "channel_count": 1 })),
    );
    assert_eq!(swapped, ["mix"]);
}

#[test]
fn a_channel_dropped_with_its_input_module_fades_out() {
    let swapped = a_removal_does_not_step(
        document(json!({ "channel_count": 2 })),
        without_high(document(json!({ "channel_count": 1 }))),
    );
    assert_eq!(swapped, ["mix"]);
}

#[test]
fn a_channel_whose_input_module_goes_fades_out() {
    let mixer = json!({ "channel_count": 2 });
    let swapped = a_removal_does_not_step(document(mixer.clone()), without_high(document(mixer)));
    assert!(swapped.is_empty(), "the mixer itself stays: {swapped:?}");
}

#[test]
fn an_input_module_removed_just_after_a_rebuild_fades_out() {
    // The first reload rebuilds the mixer, both channels still fed; the
    // second, before a block plays, removes the second channel's tone.
    let rebuilt = json!({ "channel_count": 2, "levels": [1.0, 0.8] });
    let swapped = removals_do_not_step(
        document(json!({ "channel_count": 2 })),
        &[document(rebuilt.clone()), without_high(document(rebuilt))],
    );
    assert!(
        swapped.is_empty(),
        "the second keeps the mixer: {swapped:?}"
    );
}
