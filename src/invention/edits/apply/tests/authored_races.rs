//! Authored writes the audio thread applies while a batch is planned and
//! built: the batch's document keeps each one it does not itself write, and
//! the batch's own write follows one it does.

use super::scripted::Step;
use super::*;
use crate::ControlWriteIntent;

/// A hook making each authored write and letting the audio thread apply
/// it, a block apiece, as another client's writes land mid-batch.
fn authored(running: &RunningInvention, pump: &Pump, writes: Vec<(&'static str, f32)>) -> Step {
    let (snapshot, pump) = (running.snapshot(), pump.clone());
    Step::Run(Box::new(move || {
        for (module_id, value) in writes {
            let author = ControlWriteIntent::Author;
            let write =
                snapshot.set_control_with_intent(module_id, "frequency", number(value), author);
            write.unwrap();
            pump.block();
        }
    }))
}

/// Adds `t`, whose build runs the scripted step, and retunes osc1.
fn retuning_batch() -> Vec<StructuralEdit> {
    vec![
        add("t", SCRIPTED, json!({})),
        set("osc1", "frequency", number(220.0)),
    ]
}

fn frequency(running: &RunningInvention, id: &str) -> f64 {
    config_of(running, id)["frequency"].as_f64().unwrap()
}

#[test]
fn an_authored_write_landing_mid_batch_stays_unless_the_batch_writes_it() {
    let scripted = Scripted::default();
    let (mut running, pump) = start_with(scripted.registry(), BASE);
    let hook = authored(&running, &pump, vec![("osc1", 330.0), ("osc2", 660.0)]);
    // Checked, validated, then the commit's build lets the writes land.
    scripted.then(None).then(None).then(Some(hook));
    let generation = running.live.generation();

    running
        .apply_edits(&retuning_batch())
        .expect("the batch commits");
    pump.block();

    // Committed on the first attempt, with no planning again.
    assert_eq!(scripted.builds(), 3);
    assert_eq!(running.live.generation(), generation + 1);
    // The batch's write follows osc1's; osc2's is kept.
    assert_eq!(frequency(&running, "osc1"), 220.0);
    assert_eq!(frequency(&running, "osc2"), 660.0);
    assert_eq!(
        running.get_control("osc1", "frequency").unwrap(),
        number(220.0)
    );
    assert_eq!(
        running.get_control("osc2", "frequency").unwrap(),
        number(660.0)
    );
    let state = running.state.lock().unwrap();
    assert_eq!(state.modules["osc2"].config["frequency"], json!(660));
    assert!(state
        .document()
        .unwrap()
        .modules
        .iter()
        .any(|m| m.id == "t"));
}

#[test]
fn a_batch_more_authored_writes_landed_under_than_are_kept_is_planned_again() {
    let scripted = Scripted::default();
    let (mut running, pump) = start_with(scripted.registry(), BASE);
    let flood = (0..65).map(|step| ("osc2", 600.0 + step as f32)).collect();
    let hook = authored(&running, &pump, flood);
    scripted.then(None).then(None).then(Some(hook));

    running
        .apply_edits(&retuning_batch())
        .expect("the batch commits");
    pump.block();

    // Refused once, then checked, validated and built again from the
    // document holding them all.
    assert_eq!(scripted.builds(), 6);
    assert_eq!(frequency(&running, "osc1"), 220.0);
    assert_eq!(frequency(&running, "osc2"), 664.0);
}

#[test]
fn a_write_still_pending_at_the_commit_follows_one_the_commit_records_again() {
    let scripted = Scripted::default();
    let (mut running, pump) = start_with(scripted.registry(), BASE);
    let (snapshot, hook_pump) = (running.snapshot(), pump.clone());
    let hook = Step::Run(Box::new(move || {
        let author = ControlWriteIntent::Author;
        let write =
            |value| snapshot.set_control_with_intent("osc2", "frequency", number(value), author);
        write(660.0).unwrap();
        hook_pump.block();
        // Not yet taken by the audio thread when the batch commits.
        write(770.0).unwrap();
    }));
    scripted.then(None).then(None).then(Some(hook));

    running
        .apply_edits(&retuning_batch())
        .expect("the batch commits");
    pump.block();
    // Recorded as the publisher is next locked.
    running.live.authored_epoch();

    assert_eq!(frequency(&running, "osc2"), 770.0);
    assert_eq!(
        running.get_control("osc2", "frequency").unwrap(),
        number(770.0)
    );
}

#[test]
fn an_authored_write_to_a_module_the_batch_rebuilds_is_not_recorded_again() {
    let scripted = Scripted::default();
    let (mut running, pump) = start_with(scripted.registry(), BASE);
    let hook = authored(&running, &pump, vec![("osc2", 660.0)]);
    scripted.then(None).then(None).then(Some(hook));

    let rebuilt = json!({ "waveform": "sine", "frequency": 330.0 });
    running
        .apply_edits(&[
            add("t", SCRIPTED, json!({})),
            remove("osc2"),
            add("osc2", "oscillator", rebuilt),
            connect("osc2", "audio", "dac", "audio"),
        ])
        .expect("the batch commits");
    pump.block();

    // The module playing was built from the batch's document.
    assert_eq!(scripted.builds(), 3);
    assert_eq!(frequency(&running, "osc2"), 330.0);
    assert_eq!(
        running.get_control("osc2", "frequency").unwrap(),
        number(330.0)
    );
}

#[test]
fn a_timed_batch_an_authored_write_landed_under_keeps_it_as_it_lands() {
    let scripted = Scripted::default();
    let (mut running, pump) = start_with(scripted.registry(), BASE);
    let hook = authored(&running, &pump, vec![("osc2", 660.0)]);
    scripted.then(None).then(None).then(Some(hook));

    let when = crate::rpc::EditWhen::AfterSamples { samples: 64 };
    running
        .apply_edits_at(&retuning_batch(), &when, None)
        .expect("the batch is queued");

    // Queued on the first attempt, with the write folded into its document.
    assert_eq!(scripted.builds(), 3);
    pump.render(4);
    running.live.adopt_landed();
    assert_eq!(frequency(&running, "osc1"), 220.0);
    assert_eq!(frequency(&running, "osc2"), 660.0);
}
