//! Structural edits timed like requests, on a live graph: each lands on the
//! exact sample its time resolves to, a twin clock the oracle; one that
//! another change overtakes is refused, never applied to a graph it was not
//! prepared for; its document change is recorded as it is adopted; every
//! block stays allocation- and free-free.

use super::request_beats::{
    beat_rig, blocks_for, level_change, outcomes_of, render_counted, Twin, BPM,
};
use super::request_conditions::{first_change, nth_beat, rig as condition_rig};
use super::*;
use crate::control_request::{BeatSpec, Edge, Outcome, Refusal, RequestId, RtValue, Timeline};
use crate::invention::publish::timed::EditTime;
use crate::invention::publish::watch::Part;

/// Removes the dial (level 0.25) from a rig built by [`beat_rig`] at `time`:
/// from the sample it lands on, the output is the clock's gate alone.
fn remove_dial(rig: &Rig, time: EditTime<'_>) -> Result<RequestId, GraphCommandError> {
    let mut change = rig.live.begin();
    change.remove("dial");
    let prepared = change.prepare().unwrap();
    rig.live.commit_when(prepared, time, None, |state| {
        state.document = None;
    })
}

/// Where the dial stops being heard in `out`, from sample `start`.
fn dial_gone(out: &[f32], start: u64) -> Option<u64> {
    level_change(out, 0.0).map(|i| start + i as u64)
}

#[test]
fn an_edit_n_beats_ahead_lands_on_the_beats_exact_sample() {
    for bpm in [120.0, 97.0, 133.3] {
        for (blocks, beats) in [(1, 4.0), (7, 2.0), (40, 1.5)] {
            let mut rig = beat_rig(bpm);
            rig.render(blocks);
            let start = rig.graph.current_sample;
            let spec = BeatSpec::After(beats);
            let id = remove_dial(
                &rig,
                EditTime::Beat {
                    clock: "clock",
                    spec,
                },
            )
            .unwrap();
            let out = render_counted(&mut rig, blocks_for(bpm, f64::from(beats)));

            let mut twin = Twin::new(bpm);
            twin.run_to(start);
            let goal = twin.clock.position().max(0.0) + f64::from(beats);
            let at = twin.until(|clock| clock.position() >= goal);
            let landed = Outcome::Applied { at };
            assert_eq!(outcomes_of(&mut rig, id), [(id, landed)], "{bpm}, +{beats}");
            assert_eq!(dial_gone(&out, start), Some(at), "{bpm} bpm, +{beats}");
            assert_eq!(rig.module_ids(), ["clock", "dac"]);
        }
    }
}

#[test]
fn an_edit_at_a_sample_lands_on_it_mid_block() {
    let mut rig = beat_rig(120.0);
    rig.render(3);
    let start = rig.graph.current_sample;
    let at = start + 64 * 5 + 17;
    let id = remove_dial(&rig, EditTime::AtSample(at)).unwrap();
    let out = render_counted(&mut rig, 8);
    assert_eq!(outcomes_of(&mut rig, id), [(id, Outcome::Applied { at })]);
    assert_eq!(dial_gone(&out, start), Some(at));
    // One commit more in the request log (the rig made one), none lost.
    let log = rig.live.request_log();
    assert_eq!((log.edits, log.lost), (2, 0));
}

#[test]
fn an_edit_reporting_after_the_next_is_queued_is_not_counted_lost() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    let first = remove_dial(&rig, EditTime::AfterSamples(10)).unwrap();
    render_counted(&mut rig, 1);
    // Standing in for the next edit, registered before the first's
    // outcome is received (adopted as it landed, its outcome in flight).
    rig.live
        .pending
        .lock()
        .unwrap()
        .timed(RequestId(first.0 + 1_000));
    let log = rig.live.request_log();
    assert_eq!((log.edits, log.lost), (3, 0));
}

#[test]
fn untouched_modules_play_as_a_twin_that_never_edited() {
    let mut edited = Rig::new(BASE);
    let mut untouched = Rig::new(BASE);
    assert_eq!(edited.render(3), untouched.render(3));

    // An unconnected module, added mid-block: every audible module survives.
    let lfo = edited.build("lfo", "oscillator", serde_json::json!({ "frequency": 2.0 }));
    let mut change = edited.live.begin();
    change.upsert("lfo", lfo);
    let prepared = change.prepare().unwrap();
    let at = edited.graph.current_sample + 100;
    let id = edited
        .live
        .commit_when(prepared, EditTime::AtSample(at), None, |_| {})
        .unwrap();

    assert_eq!(render_counted(&mut edited, 20), untouched.render(20));
    assert_eq!(
        outcomes_of(&mut edited, id),
        [(id, Outcome::Applied { at })]
    );
    assert_eq!(edited.module_ids(), ["osc1", "osc2", "dac", "lfo"]);
}

#[test]
fn the_document_change_is_recorded_as_the_edit_is_adopted() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    let before = snapshot(&rig);
    assert!(before.2.is_some());
    let at = rig.graph.current_sample + 64 * 2 + 5;
    remove_dial(&rig, EditTime::AtSample(at)).unwrap();
    // Waiting: the runtime still describes the graph that is playing.
    assert_eq!(snapshot(&rig).0, before.0);
    assert!(rig.live.state.lock().unwrap().document.is_some());

    render_counted(&mut rig, 4);
    // Landed, and adopted by the next call that takes the publisher.
    assert_eq!(rig.live.generation(), before.5 .0 + 1);
    let (modules, _, document, surfaces, ports, _) = snapshot(&rig);
    assert!(document.is_none(), "the retain ran");
    assert!(!modules.contains_key("dial"));
    assert!(!surfaces.contains(&"dial".to_string()));
    assert!(!ports.contains(&"dial".to_string()));
}

#[test]
fn a_change_published_while_an_edit_waits_refuses_it() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    let at = rig.graph.current_sample + 64 * 4;
    let id = remove_dial(&rig, EditTime::AtSample(at)).unwrap();
    // Prepared against the same graph, published first: it stands.
    rig.live
        .edit(|change| {
            change.remove("clock");
            Ok(())
        })
        .unwrap();

    render_counted(&mut rig, 8);
    let refused = Outcome::Refused(Refusal::TargetGone);
    assert_eq!(outcomes_of(&mut rig, id), [(id, refused)]);
    assert_eq!(rig.module_ids(), ["dac", "dial"]);
    // The publisher forgot it: another can be timed.
    let id = remove_dial(&rig, EditTime::AfterSamples(10)).unwrap();
    render_counted(&mut rig, 2);
    let landed = outcomes_of(&mut rig, id);
    assert!(matches!(landed[..], [(_, Outcome::Applied { .. })]));
    assert_eq!(rig.module_ids(), ["dac"]);
}

#[test]
fn writes_between_landing_and_adoption_reach_the_new_graph() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    remove_dial(&rig, EditTime::AfterSamples(10)).unwrap();
    render_counted(&mut rig, 1);
    // Resolved against the graph the edit replaced, still the publisher's,
    // and mapped through the landing to the clock that survived it.
    let at = rig.graph.current_sample + 3;
    super::request_beats::submit_clock_at(&rig, BPM, RtValue::F32(240.0), at);
    render_counted(&mut rig, 2);
    let clock = rig.graph.modules.get_mut("clock").unwrap().module_mut();
    assert_eq!(clock.get_control("bpm").unwrap(), 240.0);
    // Adopted; the bridge goes back once nothing is tagged older.
    assert_eq!(rig.live.generation(), 2);
    render_counted(&mut rig, 2);
}

#[test]
fn an_edit_timed_by_a_condition_lands_by_the_segment_after_it_is_met() {
    let oracle = condition_rig(true).render(200);
    let mut rig = condition_rig(true);
    let parts = [Part::Signal {
        module: "clock",
        port: "beat",
        edge: Edge::Rising,
        level: 0.5,
        count: 2,
    }];
    let id = remove_dial(&rig, EditTime::Condition(&parts)).unwrap();
    let out = render_counted(&mut rig, 200);

    let beat = nth_beat(&oracle, 2);
    let gone = first_change(&out, &oracle);
    let outcome = outcomes_of(&mut rig, id);
    match outcome[..] {
        [(_, Outcome::Applied { at })] => assert_eq!((at, gone), (beat, beat)),
        [(_, Outcome::AppliedLate { at, due })] => {
            assert_eq!((due, at), (beat, gone));
            assert!(at - beat < 64, "landed {} samples late", at - beat);
        }
        _ => panic!("{outcome:?}"),
    }
}

#[test]
fn what_cannot_be_timed_is_refused_with_nothing_queued() {
    let rig = beat_rig(120.0);
    // Edits nothing.
    let empty = rig.live.begin().prepare().unwrap();
    let error = rig
        .live
        .commit_when(empty, EditTime::AfterSamples(10), None, |_| {})
        .unwrap_err();
    assert!(
        matches!(error, GraphCommandError::ControlError(_)),
        "{error}"
    );
    // An unknown clock.
    let unknown = EditTime::Beat {
        clock: "metronome",
        spec: BeatSpec::After(1.0),
    };
    assert!(remove_dial(&rig, unknown).is_err());
    // Removing a module that runs a script, standing in: only its type is
    // read.
    let script = crate::RuntimeModuleInfo {
        id: "dial".into(),
        module_type: "code".into(),
        config: serde_json::json!({}),
    };
    let mut state = rig.live.state.lock().unwrap();
    state.modules.insert("dial".into(), script);
    drop(state);
    let error = remove_dial(&rig, EditTime::AfterSamples(10)).unwrap_err();
    assert!(error.to_string().contains("code or agent"), "{error}");
    assert!(!rig.live.publisher().lock().unwrap().timed_waiting());
}

#[test]
fn an_edit_sharing_a_segment_with_automation_ramps_moves_nothing_it_keeps() {
    use super::migrated::block_with_locks_held;
    use super::request_tiers::{level, ramp_rig, step};
    use crate::control_request::{Request, RequestValue, Source, When};
    use crate::test_support::dial::LEVEL;

    let (mut edited, mut twin) = (ramp_rig(), ramp_rig());
    assert_eq!(
        (step(&mut edited), step(&mut edited)),
        (step(&mut twin), step(&mut twin))
    );
    // An automation-tier write and the edit, due on one sample mid-block,
    // while the scheduler ramps the dial. The edit removes the dac ahead of
    // the dial and the scheduler, so both move.
    let at = edited.graph.current_sample + 64 + 20;
    for rig in [&edited, &twin] {
        let publisher = rig.live.publisher().lock().unwrap();
        let target = publisher.control_target("dial", LEVEL).unwrap();
        let mut request = Request::new(target, RequestValue::Value(RtValue::F32(0.3)));
        (request.when, request.source) = (When::AtSample(at), Source::Automation);
        rig.live.requests.submit(request).unwrap();
    }
    let mut change = edited.live.begin();
    change.remove("dac");
    let prepared = change.prepare().unwrap();
    let id = edited
        .live
        .commit_when(prepared, EditTime::AtSample(at), None, |_| {})
        .unwrap();

    for _ in 0..6 {
        assert_eq!(step(&mut edited), step(&mut twin));
    }
    let landed = outcomes_of(&mut edited, id);
    assert_eq!(landed, [(id, Outcome::Applied { at })]);
    assert_eq!(edited.module_ids(), ["dial", "sched"]);
    let (allocs, frees) = block_with_locks_held(&mut edited, &[]).unwrap();
    twin.render(1);
    assert_eq!((allocs, frees, level(&edited)), (0, 0, level(&twin)));
}

#[test]
fn a_landing_block_takes_no_lock() {
    use super::migrated::block_with_locks_held;

    let mut rig = beat_rig(120.0);
    rig.render(1);
    let at = rig.graph.current_sample + 30;
    let id = remove_dial(&rig, EditTime::AtSample(at)).unwrap();
    // Every lock a control thread takes is held while the edit lands.
    let (allocs, frees) = block_with_locks_held(&mut rig, &[]).unwrap();
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(outcomes_of(&mut rig, id), [(id, Outcome::Applied { at })]);
}

#[test]
fn the_reclaimer_thread_adopts_a_landed_edit_on_its_own() {
    let mut rig = beat_rig(120.0);
    assert!(rig.live.start_reclaimer());
    rig.render(1);
    let id = remove_dial(&rig, EditTime::AfterSamples(10)).unwrap();
    render_counted(&mut rig, 1);

    // Nothing takes the publisher: the reclaimer's cadence adopts it.
    let recorded = || rig.live.state.lock().unwrap().document.is_none();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !recorded() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(recorded(), "adopted within a few reclaim intervals");
    assert!(!rig.live.state.lock().unwrap().modules.contains_key("dial"));
    assert!(!rig.live.publisher().lock().unwrap().timed_waiting());
    assert!(matches!(
        outcomes_of(&mut rig, id)[..],
        [(_, Outcome::Applied { .. })]
    ));
}
