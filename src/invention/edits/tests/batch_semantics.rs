//! What a batch reports about modules it removes, replaces or writes to
//! more than once.

use super::*;
use crate::rpc::MAX_EDITS_PER_BATCH;

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
        set("osc2", "rate", ControlValue::Number(0.5)),
    ])
    .unwrap();
    assert_eq!(
        writes(&candidate.control_writes),
        [(4, "osc2.rate".into(), ControlValue::Number(0.5))]
    );
    assert_eq!(
        config_of(&candidate.document, "osc2"),
        &json!({ "rate": 0.5 })
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

#[test]
fn a_module_added_and_removed_in_one_batch_leaves_nothing_kept() {
    let document = base();
    let mut facts = FakeFacts::for_document(&document);
    let candidate = apply_to_candidate(
        &document,
        &[add("lfo", "lfo", json!({ "rate": 1 })), remove("lfo")],
        &mut facts,
    )
    .unwrap();
    assert_eq!(candidate.document, document);
    assert_eq!(facts.described, ["lfo"]);
    assert!(facts.kept.is_empty());
}

#[test]
fn the_latest_describe_for_an_id_wins() {
    let document = base();
    let mut facts = FakeFacts::for_document(&document);
    let candidate = apply_to_candidate(
        &document,
        &[
            add("mod", "oscillator", json!({ "frequency": 1 })),
            remove("mod"),
            add("mod", "lfo", json!({ "rate": 2 })),
            // The lfo's port, not the oscillator's: the overlay holds the
            // latest facts.
            connect("mod", "bipolar", "osc1", "fm"),
        ],
        &mut facts,
    )
    .unwrap();
    assert_eq!(facts.described, ["mod", "mod"]);
    assert_eq!(
        facts.kept,
        HashMap::from([("mod".to_string(), json!({ "rate": 2 }))])
    );
    assert!(candidate.replaced.is_empty());

    let failure = apply_to_candidate(
        &document,
        &[
            add("mod", "oscillator", json!(null)),
            remove("mod"),
            add("mod", "lfo", json!(null)),
            connect("mod", "audio", "dac", "audio"),
        ],
        &mut FakeFacts::for_document(&document),
    )
    .unwrap_err();
    assert_eq!(
        (failure.index, failure.reason),
        (3, EditFailureReason::UnknownPort)
    );
}

#[test]
fn non_finite_numbers_are_refused() {
    for value in [
        ControlValue::Number(f32::NAN),
        ControlValue::Number(f32::INFINITY),
        ControlValue::Number(f32::NEG_INFINITY),
        ControlValue::String("nan".into()),
        ControlValue::String("inf".into()),
        ControlValue::String("-infinity".into()),
        ControlValue::String("1e39".into()),
    ] {
        let failure = refused(vec![set("osc1", "frequency", value.clone())]);
        assert_eq!(
            failure.reason,
            EditFailureReason::InvalidControlValue,
            "{value:?}"
        );
        assert!(failure.message.contains("finite"), "{}", failure.message);
    }

    // A wire number too large for an f32 arrives as infinity.
    let edit: StructuralEdit = serde_json::from_value(json!({
        "op": "set_control", "module_id": "osc1", "key": "frequency", "value": 1e39
    }))
    .unwrap();
    let failure = refused(vec![edit]);
    assert_eq!(failure.reason, EditFailureReason::InvalidControlValue);
    assert!(failure.message.contains("finite"), "{}", failure.message);

    // A value of the wrong kind is refused for its kind, finite or not.
    let edit: StructuralEdit = serde_json::from_value(json!({
        "op": "set_control", "module_id": "lfo", "key": "retrigger", "value": 1e39
    }))
    .unwrap();
    let failure = refused(vec![add("lfo", "lfo", json!(null)), edit]);
    assert_eq!(
        (failure.index, failure.reason),
        (1, EditFailureReason::InvalidControlValue)
    );
    assert!(
        failure.message.contains("expects a boolean"),
        "{}",
        failure.message
    );

    // The largest finite f32 is a number like any other.
    let candidate = apply(vec![set(
        "osc1",
        "frequency",
        ControlValue::Number(f32::MAX),
    )])
    .unwrap();
    assert_eq!(candidate.control_writes.len(), 1);
}

#[test]
fn refusals_echo_a_bounded_part_of_the_value() {
    let huge = "é".repeat(100_000);
    let failure = refused(vec![set(
        "osc1",
        "frequency",
        ControlValue::String(huge.clone()),
    )]);
    assert_eq!(failure.reason, EditFailureReason::InvalidControlValue);
    assert!(failure.message.len() < 200, "{}", failure.message);
    assert!(failure.message.contains("éé…"), "{}", failure.message);

    // A module that echoes the config it refused is cut too.
    let failure = refused(vec![add("bad", "lfo", json!({ "broken": huge }))]);
    assert_eq!(failure.reason, EditFailureReason::InvalidConfig);
    assert!(failure.message.len() < 400, "{}", failure.message);
    assert!(failure.message.ends_with('…'));

    // A short value is shown whole.
    let failure = refused(vec![set("osc1", "frequency", ControlValue::Bool(true))]);
    assert!(failure.message.ends_with("got true"), "{}", failure.message);
}

#[test]
fn a_number_sent_to_a_string_control_becomes_its_text() {
    // Coerced as a standalone write coerces it. Whether the module offers
    // that option is the module's own check, made at commit.
    let candidate = apply(vec![set("osc1", "type", ControlValue::Number(3.0))]).unwrap();
    assert_eq!(
        writes(&candidate.control_writes),
        [(0, "osc1.type".into(), ControlValue::String("3".into()))]
    );
    assert_eq!(
        config_of(&candidate.document, "osc1"),
        &json!({ "frequency": 440, "type": "3" })
    );
}

#[test]
fn a_refusal_at_the_batch_limit_names_index_255() {
    let mut edits: Vec<StructuralEdit> = (0..MAX_EDITS_PER_BATCH - 1)
        .map(|step| set("osc1", "frequency", ControlValue::Number(step as f32)))
        .collect();
    edits.push(connect("osc1", "audio", "osc2", "nowhere"));
    assert_eq!(edits.len(), MAX_EDITS_PER_BATCH);
    let failure = refused(edits);
    assert_eq!(
        (failure.index, failure.op, failure.reason),
        (255, EditOp::Connect, EditFailureReason::UnknownPort)
    );
}
