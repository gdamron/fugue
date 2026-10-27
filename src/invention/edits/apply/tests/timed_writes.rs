//! A timed authored write waiting as a batch commits: whichever of it and
//! the batch's write to its control applies later is what plays, and the
//! document records the same, with or without a publication and whether it
//! applies before the batch is planned, while it is, or after it commits.
//! A batch rebuilding its module replaces it in both, and one the audio
//! thread applies just before the batch's own write is recorded before it;
//! one submitted as a batch or a reload commits waits for its writes.
//! One landing after a whole scale written before it is recorded as a cold
//! build reads it, and one against a timed batch writing its control is
//! recorded in the order each applied, the batch's write included, or not
//! at all when the write outranks it on its sample, a whole scale over a
//! degree included.

use super::real_modules::assert_document_rebuilds_the_live_melody;
use super::scripted::Step;
use super::*;
use crate::control_request::Source;
use crate::invention::declared::WriteOptions;
use crate::invention::orchestration::RuntimeSnapshot;
use crate::invention::publish::{Contested, WriteTime};
use crate::ControlWriteIntent;

/// Authors `value` for `key` of `module_id` on `sample`.
fn author(
    running: &RunningInvention,
    (module_id, key): (&str, &str),
    value: ControlValue,
    sample: u64,
) {
    author_ranked(running, (module_id, key), value, (sample, 0));
}

/// [`author`], at `priority`.
fn author_ranked(
    running: &RunningInvention,
    (module_id, key): (&str, &str),
    value: ControlValue,
    (sample, priority): (u64, i16),
) {
    let time = (WriteTime::AtSample(sample), priority);
    author_through(&running.snapshot(), (module_id, key), value, time);
}

fn author_through(
    snapshot: &RuntimeSnapshot,
    (module_id, key): (&str, &str),
    value: ControlValue,
    (time, priority): (WriteTime<'_>, i16),
) {
    let options = WriteOptions {
        time: Some(time),
        priority,
        ..WriteOptions::performed(Source::User)
    };
    let author = ControlWriteIntent::Author;
    let ack = snapshot.submit_control(module_id, key, value, author, options);
    assert!(ack.unwrap().request.is_some(), "submitted as a request");
}

/// Authors `osc`'s frequency `value` on `sample`.
fn author_at(running: &RunningInvention, osc: &str, value: f32, sample: u64) {
    author(running, (osc, "frequency"), number(value), sample);
}

/// Its `key` as the document records it and as it plays.
fn heard_and_recorded_key(
    running: &RunningInvention,
    (osc, key): (&str, &str),
) -> (serde_json::Value, ControlValue) {
    let document = running.document().unwrap();
    let spec = document.modules.iter().find(|spec| spec.id == osc).unwrap();
    let recorded = spec.config[key].clone();
    (recorded, running.get_control(osc, key).unwrap())
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
fn a_contested_commit_write_is_recorded_after_a_timed_write_applied_before_it() {
    let writes = [("osc1".to_string(), "frequency".to_string())];
    for contested in [true, false] {
        let (running, pump) = start(BASE);
        author_at(&running, "osc1", 330.0, 64 * 2 + 7);
        // Applied on the audio thread, its outcome not yet taken, as a
        // commit records a document of its own retuning osc1: the race a
        // device stream can run, here in order.
        pump.render(4);
        let mut state = running.state.lock().unwrap();
        state.document_write_control("osc1", "frequency", &number(220.0));
        drop(state);
        let commit = match contested {
            true => running.live.contested(&[], &writes),
            false => Contested::default(),
        };
        // Recorded by a read before the commit's write is made: what the
        // commit records was read as it landed, so it is not taken for it.
        running.document().unwrap();
        let snapshot = running.snapshot();
        let control = ("osc1", "frequency");
        snapshot
            .set_control_committed(control, number(220.0), &commit)
            .unwrap();
        pump.render(2);
        let (recorded, heard) = heard_and_recorded(&running, "osc1");
        assert_eq!(heard, number(220.0), "the commit's write applied last");
        // Uncontested, the timed write is recorded over the commit's
        // document, though the commit's write plays: why a commit reports
        // the write, so it is recorded after.
        let expected = if contested { 220.0 } else { 330.0 };
        assert_eq!(recorded, expected, "contested: {contested}");
    }
}

#[test]
fn a_timed_write_applied_and_not_yet_recorded_contests_a_commit() {
    let (running, pump) = start(BASE);
    let writes = [("osc1".to_string(), "frequency".to_string())];
    author_at(&running, "osc1", 330.0, 64 * 2 + 7);
    assert!(running
        .live
        .contested(&[], &writes)
        .authoring("osc1", "frequency")
        .is_some());
    // Its outcome taken by an announcement, outside the publisher: listed
    // for the document, not yet recorded in it.
    pump.render(4);
    running.announce_applied();
    let contested = running.live.contested(&[], &writes);
    let authoring = contested.authoring("osc1", "frequency").unwrap();
    assert_eq!(
        authoring.value,
        number(440.0),
        "the document's, not the write's"
    );
    // A module the commit stopped is never contested.
    let stopped = ["osc1".to_string()];
    assert!(running
        .live
        .contested(&stopped, &writes)
        .authoring("osc1", "frequency")
        .is_none());
}

#[test]
fn a_timed_write_submitted_as_a_commit_lands_waits_for_its_writes() {
    use std::sync::{mpsc, Arc, Mutex};
    // A batch with a publication or none, and a reload carrying the value.
    for commit in 0..3 {
        let (mut running, pump) = start(BASE);
        let snapshot = running.snapshot();
        let writer = Arc::new(Mutex::new(None));
        let joined = writer.clone();
        // As the commit releases the publisher, before its writes are made:
        // due at once, it would apply before them, unrecorded behind it.
        crate::invention::publish::tests::on_commit(move || {
            let (sent, submitted) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                let soon = WriteTime::AfterSamples(0);
                author_through(&snapshot, ("osc1", "frequency"), number(330.0), (soon, 0));
                let _ = sent.send(());
            });
            let waited = submitted.recv_timeout(std::time::Duration::from_millis(100));
            assert!(waited.is_err(), "submitted inside the commit");
            *writer.lock().unwrap() = Some(thread);
        });
        if commit < 2 {
            running.apply_edits(&retune(commit == 1)).unwrap();
        } else {
            let retuned = BASE.replacen("440.0", "220.0", 1);
            running
                .reload(Invention::from_json(&retuned).unwrap())
                .unwrap();
        }
        let thread = joined.lock().unwrap().take().expect("the commit ran");
        thread.join().unwrap();
        pump.render(4);
        let both = heard_and_recorded(&running, "osc1");
        assert_eq!(both, (330.0, number(330.0)), "commit {commit}");
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

#[test]
fn a_timed_choice_and_a_timed_batch_choosing_it_are_recorded_as_they_play() {
    let at = 64 * 20 + 7;
    let edit_at = crate::rpc::EditWhen::AtSample { sample: at };
    let square = vec![
        add("extra", "oscillator", json!({})),
        set("osc1", "waveform", ControlValue::String("square".into())),
    ];
    let triangle = ControlValue::String("triangle".into());
    for write_at in [at, at + 64 * 4] {
        let (mut running, pump) = start(BASE);
        running.apply_edits_at(&square, &edit_at, None).unwrap();
        author(&running, ("osc1", "waveform"), triangle.clone(), write_at);
        pump.render(40);
        let (recorded, heard) = heard_and_recorded_key(&running, ("osc1", "waveform"));
        let heard = match heard {
            ControlValue::String(text) => serde_json::Value::String(text),
            other => panic!("a choice plays as text, not {other:?}"),
        };
        assert_eq!(recorded, heard, "at {write_at}");
    }
}

#[test]
fn a_timed_write_outranking_a_timed_batch_on_its_sample_is_recorded_as_it_plays() {
    let at = 64 * 20 + 7;
    let edit_at = crate::rpc::EditWhen::AtSample { sample: at };
    // Outranking the batch's write, it wins the sample: the batch's never
    // applies. Outranked, the batch's write wins it.
    for (priority, heard) in [(1, 330.0), (-1, 220.0)] {
        let (mut running, pump) = start(BASE);
        let write = (at, priority);
        author_ranked(&running, ("osc1", "frequency"), number(330.0), write);
        running
            .apply_edits_at(&retune(true), &edit_at, None)
            .unwrap();
        pump.render(40);
        let both = heard_and_recorded(&running, "osc1");
        assert_eq!(both, (heard, number(heard as f32)), "priority {priority}");
    }
}

#[test]
fn a_timed_scale_outranking_a_timed_batch_degree_on_its_sample_is_recorded_as_it_plays() {
    let at = 64 * 20 + 7;
    let edit_at = crate::rpc::EditWhen::AtSample { sample: at };
    let degree = vec![
        add("extra", "oscillator", json!({})),
        set("tune", "degree.1", number(9.0)),
    ];
    let scale = ControlValue::String("[0, 5, 7]".into());
    // Outranking the batch's degree, the whole scale holds every degree on
    // the sample: the batch's never applies. Outranked, the degree follows.
    for (priority, heard) in [(1, 5.0), (-1, 9.0)] {
        let (mut running, pump) = start(&base_with(
            r#"{ "id": "tune", "type": "melody", "config": { "degrees": [0, 2, 4] } }"#,
        ));
        let write = (at, priority);
        author_ranked(&running, ("tune", "degrees"), scale.clone(), write);
        running.apply_edits_at(&degree, &edit_at, None).unwrap();
        pump.render(40);
        let played = running.get_control("tune", "degree.1").unwrap();
        assert_eq!(played, number(heard), "priority {priority}");
        assert_document_rebuilds_the_live_melody(&running);
    }
}
