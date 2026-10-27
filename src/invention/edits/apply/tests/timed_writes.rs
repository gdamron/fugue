//! A timed authored write waiting as a batch commits: whichever of it and
//! the batch's write to its control applies later is what plays, and the
//! document records the same, with or without a publication and whether it
//! applies before the batch is planned, while it is, or after it commits.
//! A batch rebuilding its module replaces it in both. One landing after a
//! whole scale written before it is recorded as a cold build reads it, and one
//! against a timed batch writing its control is recorded in the order each
//! applied, the batch's write included.

use super::real_modules::assert_document_rebuilds_the_live_melody;
use super::scripted::Step;
use super::*;
use crate::control_request::Source;
use crate::invention::declared::WriteOptions;
use crate::invention::publish::WriteTime;
use crate::ControlWriteIntent;

/// Authors `value` for `key` of `module_id` on `sample`.
fn author(
    running: &RunningInvention,
    (module_id, key): (&str, &str),
    value: ControlValue,
    sample: u64,
) {
    let options = WriteOptions {
        time: Some(WriteTime::AtSample(sample)),
        ..WriteOptions::performed(Source::User)
    };
    let author = ControlWriteIntent::Author;
    let ack = running
        .snapshot()
        .submit_control(module_id, key, value, author, options);
    assert!(ack.unwrap().request.is_some(), "submitted as a request");
}

/// Authors `osc`'s frequency `value` on `sample`.
fn author_at(running: &RunningInvention, osc: &str, value: f32, sample: u64) {
    author(running, (osc, "frequency"), number(value), sample);
}

/// Its frequency as the document records it and as it plays.
fn heard_and_recorded(running: &RunningInvention, osc: &str) -> (f64, ControlValue) {
    let document = running.document().unwrap();
    let spec = document.modules.iter().find(|spec| spec.id == osc).unwrap();
    let recorded = spec.config["frequency"].as_f64().unwrap();
    (recorded, running.get_control(osc, "frequency").unwrap())
}

/// Retunes osc1, with a module added (a publication) or without.
fn retune(published: bool) -> Vec<StructuralEdit> {
    let mut batch = vec![set("osc1", "frequency", number(220.0))];
    if published {
        batch.insert(0, add("extra", "oscillator", json!({})));
    }
    batch
}

#[test]
fn a_timed_write_landing_after_a_batch_writes_its_control_plays_and_is_recorded() {
    for published in [false, true] {
        let (mut running, pump) = start(BASE);
        author_at(&running, "osc1", 330.0, 64 * 20 + 7);
        running.apply_edits(&retune(published)).unwrap();
        pump.render(2);
        assert_eq!(heard_and_recorded(&running, "osc1"), (220.0, number(220.0)));
        pump.render(30);
        let both = heard_and_recorded(&running, "osc1");
        assert_eq!(both, (330.0, number(330.0)), "published: {published}");
    }
}

#[test]
fn a_timed_write_applied_before_a_batch_writes_its_control_gives_way_to_it() {
    for published in [false, true] {
        let (mut running, pump) = start(BASE);
        author_at(&running, "osc1", 330.0, 64 * 2 + 7);
        // Applied, its outcome not yet taken as the batch is planned.
        pump.render(4);
        running.apply_edits(&retune(published)).unwrap();
        pump.render(2);
        let both = heard_and_recorded(&running, "osc1");
        assert_eq!(both, (220.0, number(220.0)), "published: {published}");
    }
}

#[test]
fn a_timed_write_applied_while_a_batch_is_planned_gives_way_to_it() {
    let scripted = Scripted::default();
    let (mut running, pump) = start_with(scripted.registry(), BASE);
    let window = pump.clone();
    author_at(&running, "osc1", 330.0, 64 * 2 + 7);
    // Checked, validated, then the commit's build lets it apply.
    let hook = Step::Run(Box::new(move || window.render(4).truncate(0)));
    scripted.then(None).then(None).then(Some(hook));
    let mut batch = retune(false);
    batch.insert(0, add("t", SCRIPTED, json!({})));
    running.apply_edits(&batch).unwrap();
    pump.render(2);
    assert_eq!(heard_and_recorded(&running, "osc1"), (220.0, number(220.0)));
}

#[test]
fn a_batch_rebuilding_its_module_replaces_a_timed_write_before_or_after_it() {
    for applied_first in [false, true] {
        let (mut running, pump) = start(BASE);
        author_at(&running, "osc2", 660.0, 64 * 2 + 7);
        if applied_first {
            pump.render(4);
        }
        let rebuilt = json!({ "waveform": "square", "frequency": 330.0 });
        running
            .apply_edits(&[
                remove("osc2"),
                add("osc2", "oscillator", rebuilt),
                connect("osc2", "audio", "dac", "audio"),
            ])
            .unwrap();
        pump.render(8);
        let both = heard_and_recorded(&running, "osc2");
        assert_eq!(
            both,
            (330.0, number(330.0)),
            "applied first: {applied_first}"
        );
    }
}

#[test]
fn a_timed_degree_landing_after_a_whole_scale_is_recorded_as_a_cold_build_reads_it() {
    let (running, pump) = start(&base_with(
        r#"{ "id": "tune", "type": "melody", "config": { "degrees": [0, 2, 4] } }"#,
    ));
    author(&running, ("tune", "degree.1"), number(9.0), 64 * 20 + 7);
    // The whole scale, written at once, applies before the degree is due,
    // and would replace it, were the degree recorded as it was made.
    let author = ControlWriteIntent::Author;
    let scale = ControlValue::String("[0, 5, 7, 11]".into());
    let snapshot = running.snapshot();
    snapshot
        .set_control_with_intent("tune", "degrees", scale, author)
        .unwrap();
    pump.render(30);
    assert_eq!(
        running.get_control("tune", "degree.1").unwrap(),
        number(9.0)
    );
    assert_document_rebuilds_the_live_melody(&running);
}

#[test]
fn a_timed_write_and_a_timed_batch_writing_its_control_are_recorded_as_they_play() {
    let at = 64 * 20 + 7;
    let edit_at = crate::rpc::EditWhen::AtSample { sample: at };
    // On the batch's own sample, submitted before it or after it; then
    // after the batch's write has applied, which it follows.
    for (write_at, write_first) in [(at, true), (at, false), (at + 64 * 4, false)] {
        let (mut running, pump) = start(BASE);
        if write_first {
            author_at(&running, "osc1", 330.0, write_at);
        }
        running
            .apply_edits_at(&retune(true), &edit_at, None)
            .unwrap();
        if !write_first {
            author_at(&running, "osc1", 330.0, write_at);
        }
        pump.render(40);
        let (recorded, heard) = heard_and_recorded(&running, "osc1");
        let case = format!("at {write_at}, first: {write_first}");
        assert_eq!(number(recorded as f32), heard, "{case}");
        if write_at > at {
            assert_eq!(recorded, 330.0, "{case}");
        }
    }
}
