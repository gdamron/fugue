//! What a batch reports about modules it removes, replaces or writes to
//! more than once.

use super::*;

/// `(edit_index, "module.key", value)` for each write.
fn writes<'c>(
    writes: impl IntoIterator<Item = &'c CandidateWrite>,
) -> Vec<(usize, String, ControlValue)> {
    writes
        .into_iter()
        .map(|CandidateWrite { edit_index, write }| {
            (
                *edit_index,
                format!("{}.{}", write.module_id, write.key),
                write.value.clone(),
            )
        })
        .collect()
}

#[test]
fn a_removed_module_takes_its_earlier_writes_with_it() {
    let candidate = apply(vec![
        set("osc1", "frequency", ControlValue::Number(330.0)),
        set("osc2", "frequency", ControlValue::Number(220.0)),
        remove("osc1"),
    ])
    .unwrap();
    assert_eq!(
        writes(&candidate.control_writes),
        [(1, "osc2.frequency".into(), ControlValue::Number(220.0))]
    );
}

#[test]
fn a_replaced_module_keeps_only_the_writes_made_after_it_was_added_again() {
    let candidate = apply(vec![
        set("osc2", "frequency", ControlValue::Number(330.0)),
        set("osc2", "type", ControlValue::String("square".into())),
        remove("osc2"),
        add("osc2", "lfo", json!(null)),
        set("osc2", "frequency", ControlValue::Number(0.5)),
    ])
    .unwrap();
    assert_eq!(
        writes(&candidate.control_writes),
        [(4, "osc2.frequency".into(), ControlValue::Number(0.5))]
    );
    assert_eq!(
        config_of(&candidate.document, "osc2"),
        &json!({ "frequency": 0.5 })
    );
}

#[test]
fn final_writes_hold_one_entry_per_control_with_the_last_value() {
    let candidate = apply(vec![
        set("osc1", "frequency", ControlValue::Number(100.0)),
        set("osc2", "frequency", ControlValue::Number(200.0)),
        set("osc1", "frequency", ControlValue::Number(300.0)),
        set("osc1", "type", ControlValue::String("square".into())),
    ])
    .unwrap();
    // Every write stays in control_writes, so each is checked at commit.
    assert_eq!(candidate.control_writes.len(), 4);
    // One per control, first-written order, last value and its edit.
    assert_eq!(
        writes(candidate.final_writes()),
        [
            (2, "osc1.frequency".into(), ControlValue::Number(300.0)),
            (1, "osc2.frequency".into(), ControlValue::Number(200.0)),
            (3, "osc1.type".into(), ControlValue::String("square".into())),
        ]
    );
}

#[test]
fn removing_and_adding_an_original_id_replaces_it_even_unchanged() {
    // Same type, same config: still a fresh instance.
    let candidate = apply(vec![
        remove("osc1"),
        add("osc1", "oscillator", json!({ "frequency": 440 })),
        connect("osc1", "audio", "dac", "audio"),
    ])
    .unwrap();
    assert_eq!(
        config_of(&candidate.document, "osc1"),
        config_of(&base(), "osc1")
    );
    assert_eq!(candidate.replaced, BTreeSet::from(["osc1".to_string()]));
}

#[test]
fn only_an_original_id_that_ends_the_batch_present_is_replaced() {
    // Removed, added again, removed again: a removal, not a replacement.
    let candidate = apply(vec![
        remove("osc1"),
        add("osc1", "lfo", json!(null)),
        remove("osc1"),
    ])
    .unwrap();
    assert!(candidate.replaced.is_empty());

    // An id the batch itself added is an addition however often it cycles.
    let candidate = apply(vec![
        add("lfo", "lfo", json!(null)),
        remove("lfo"),
        add("lfo", "lfo", json!(null)),
    ])
    .unwrap();
    assert!(candidate.replaced.is_empty());
}
